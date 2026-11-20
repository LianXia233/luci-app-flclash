/* FlClash WebUI — unified communication layer.
 *
 * Responsibilities (nothing else in the UI may open a socket or poll on
 * its own):
 *   Rest      — one request pipeline: timeout, de-duplication, stale-result
 *               suppression (a slow earlier poll can never overwrite a newer
 *               one), normalised errors.
 *   Socket    — one WebSocket per tab: auth, hello/protocol negotiation,
 *               auto-reconnect with exponential backoff + jitter, heartbeat,
 *               ack/resume, subscription sync.
 *   Store     — single source of truth for every view (`state`), keyed by
 *               topic, deduplicated by sequence.
 *   Polling   — REST fallback that runs only for topics the server cannot
 *               push (or while the socket is down), and pauses when the tab
 *               is hidden.
 *   Tasks     — id → promise bridge for long-running operations.
 */
window.FlClashComm = (function () {
  'use strict';

  const cfg = window.FlClashConfig || {};
  const api = window.FlClashAPI;

  const PROTOCOL = 1;
  const RECONNECT_BASE = 1000;
  const RECONNECT_MAX = 15000;
  const ACK_EVERY_MS = 5000;
  const HEARTBEAT_TIMEOUT_MS = 50000;

  /* --------------------------------------------------------------- store */

  const state = {
    status: null,
    proxies: null,
    profiles: null,
    connections: null,
    logs: [],
    rules: null,
    config: null,
    tasks: [],
    capabilities: null,
    history: { up: [], down: [] },
    offline: false
  };

  const meta = {
    connected: false,
    connecting: false,
    transport: 'rest',
    protocol: PROTOCOL,
    bootId: '',
    lastSequence: 0,
    queued: 0,
    reconnects: 0,
    lastError: null
  };

  const specs = {
    status: {
      key: 'status', live: true, poll: 2000,
      load: function () { return api.status(); },
      apply: applyStatus
    },
    proxies: {
      key: 'proxies', live: true, poll: 3000,
      load: function () { return api.proxies(); },
      apply: function (data) { state.proxies = data || { groups: [], nodes: [] }; }
    },
    connections: {
      key: 'connections', live: true, poll: 2000,
      load: function () { return api.connections(); },
      apply: function (data) { state.connections = Array.isArray(data) ? data : []; }
    },
    logs: {
      key: 'logs', live: true, poll: 1500,
      load: function () { return api.logs(); },
      apply: function (data) { state.logs = (Array.isArray(data) ? data : []).slice(-300); }
    },
    profiles: {
      key: 'profiles', live: false, poll: 12000,
      load: function () { return api.profiles(); },
      apply: function (data) { state.profiles = Array.isArray(data) ? data : []; }
    },
    config: {
      key: 'config', live: false, poll: 12000,
      load: function () { return api.config(); },
      apply: function (data) { state.config = data || null; }
    },
    tasks: {
      key: 'tasks', live: true, poll: 4000,
      load: function () {
        return api.request('/tasks').then(function (reply) { return (reply && reply.tasks) || []; });
      },
      apply: function (data) {
        state.tasks = Array.isArray(data) ? data : [];
        resolveTaskWaiters();
      }
    }
  };

  const topicSubs = new Map();   // topic -> Set(handler)
  const metaSubs = new Set();    // transport state listeners
  const pollTimers = new Map();  // topic -> interval id
  const inflight = new Map();    // topic -> promise
  const generation = new Map();  // topic -> integer (latest wins)
  const topicSequence = new Map();
  const taskWaiters = new Map();

  /* ------------------------------------------------------------- helpers */

  function subscribers(topic) {
    let set = topicSubs.get(topic);
    if (!set) { set = new Set(); topicSubs.set(topic, set); }
    return set;
  }

  function emit(topic) {
    if (topic === 'meta') {
      const info = Object.assign({}, meta);
      metaSubs.forEach(function (handler) {
        try { handler(info); } catch (error) { console.error('[comm]', error); }
      });
      return;
    }
    const spec = specs[topic];
    if (!spec) return;
    const value = state[spec.key];
    const set = topicSubs.get(topic);
    if (!set) return;
    set.forEach(function (handler) {
      try { handler(value, topic); } catch (error) { console.error('[comm]', topic, error); }
    });
  }

  function applyStatus(status) {
    if (!status) return;
    state.status = status;
    state.offline = false;
    const traffic = status.traffic || {};
    const history = traffic.history;
    if (history && Array.isArray(history.up) && Array.isArray(history.down)) {
      state.history.up = history.up.slice(-60);
      state.history.down = history.down.slice(-60);
    }
  }

  function isNetworkError(error) {
    return !error || !error.status;
  }

  function noteError(error) {
    meta.lastError = error && error.message ? error.message : String(error);
    if (isNetworkError(error)) {
      if (!state.offline) {
        state.offline = true;
        if (window.FlClashUI && window.FlClashUI.toast) {
          window.FlClashUI.toast('后端不可达: ' + meta.lastError, 'error');
        }
      }
    }
    emit('meta');
  }

  /* ------------------------------------------------------------ REST pipe */

  async function refresh(topic, options) {
    const spec = specs[topic];
    if (!spec) return null;
    if (inflight.has(topic) && !(options && options.force === true)) return inflight.get(topic);
    const gen = (generation.get(topic) || 0) + 1;
    generation.set(topic, gen);
    const promise = spec.load()
      .then(function (data) {
        // Drop a response that a newer request has already superseded.
        if (generation.get(topic) !== gen) return state[spec.key];
        spec.apply(data);
        state.offline = false;
        emit(topic);
        return state[spec.key];
      })
      .catch(function (error) {
        noteError(error);
        throw error;
      })
      .then(function (value) {
        if (inflight.get(topic) === promise) inflight.delete(topic);
        return value;
      }, function (error) {
        if (inflight.get(topic) === promise) inflight.delete(topic);
        throw error;
      });
    inflight.set(topic, promise);
    return promise;
  }

  function startPolling(topic) {
    if (pollTimers.has(topic)) return;
    const spec = specs[topic];
    const timer = window.setInterval(function () {
      if (document.hidden) return;
      // Live topics are pushed on the socket; no point duplicating them.
      if (meta.connected && spec.live) return;
      refresh(topic).catch(function () {});
    }, spec.poll);
    pollTimers.set(topic, timer);
  }

  function stopPolling(topic) {
    const timer = pollTimers.get(topic);
    if (timer) window.clearInterval(timer);
    pollTimers.delete(topic);
  }

  /* ---------------------------------------------------------- WebSocket */

  let socket = null;
  let socketGen = 0;
  let reconnectTimer = null;
  let attempts = 0;
  let closedByAuth = false;
  let lastAckAt = 0;
  let lastServerFrame = 0;
  let watchdog = null;
  let frameQueue = Promise.resolve();
  let ackTimer = null;

  function canGzip() {
    return typeof window.DecompressionStream === 'function' && typeof window.Blob === 'function';
  }

  function wsUrl() {
    const base = String(cfg.apiBase || '/flclash/api').replace(/\/+$/, '');
    let path = base + '/ws';
    if (/^http(s)?:\/\//i.test(base)) {
      return base.replace(/^http/i, 'ws') + '/ws';
    }
    const scheme = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    return scheme + '//' + window.location.host + path;
  }

  function setTransport(value) {
    meta.transport = value;
    emit('meta');
  }

  function scheduleReconnect() {
    if (closedByAuth || cfg.mock) return;
    attempts += 1;
    const base = Math.min(RECONNECT_MAX, RECONNECT_BASE * Math.pow(2, Math.min(attempts - 1, 4)));
    const jitter = base * (0.8 + Math.random() * 0.4);
    const delay = Math.round(jitter);
    meta.connecting = true;
    meta.queued = delay;
    setTransport('reconnecting');
    if (reconnectTimer) window.clearTimeout(reconnectTimer);
    reconnectTimer = window.setTimeout(function () { reconnectTimer = null; connect(); }, delay);
  }

  function teardownSocket() {
    if (watchdog) window.clearInterval(watchdog);
    watchdog = null;
    if (ackTimer) window.clearInterval(ackTimer);
    ackTimer = null;
    if (socket) {
      try { socket.onopen = null; socket.onmessage = null; socket.onclose = null; socket.onerror = null; } catch (error) { /* ignore */ }
      try { socket.close(); } catch (error) { /* ignore */ }
    }
    socket = null;
    meta.connected = false;
  }

  function connect() {
    if (cfg.mock) { setTransport('mock'); return; }
    if (!('WebSocket' in window)) { setTransport('rest'); return; }
    if (socket && (socket.readyState === 0 || socket.readyState === 1)) return;
    clearTimeout(reconnectTimer);
    reconnectTimer = null;
    meta.connecting = true;
    const gen = ++socketGen;
    let url;
    try { url = wsUrl(); } catch (error) { setTransport('rest'); return; }
    try { socket = new WebSocket(url); } catch (error) { socket = null; scheduleReconnect(); return; }

    socket.onopen = function () {
      if (gen !== socketGen) return;
      meta.connecting = false;
      meta.connected = true;
      attempts = 0;
      lastServerFrame = Date.now();
      setTransport('websocket');
      send({ type: 'hello', protocol: PROTOCOL, capabilities: canGzip() ? ['gzip+base64'] : [], resume: meta.lastSequence, client: 'webui' });
      if (cfg.secret) send({ type: 'auth', secret: cfg.secret });
      syncSubscriptions();
    };

    socket.onmessage = function (event) {
      if (gen !== socketGen) return;
      lastServerFrame = Date.now();
      const raw = event.data;
      frameQueue = frameQueue.then(function () { return handleFrame(raw); }).catch(function (error) {
        console.warn('[comm] frame error', error);
      });
    };

    socket.onerror = function () { /* close follows */ };

    socket.onclose = function (event) {
      if (gen !== socketGen) return;
      socket = null;
      meta.connected = false;
      meta.connecting = false;
      if (event && event.code === 4401) {
        closedByAuth = true;
        meta.lastError = 'WebSocket 认证失败';
        setTransport('rest');
        if (window.FlClashUI) window.FlClashUI.toast('实时通道认证失败，已回退到轮询', 'error');
        return;
      }
      if (event && (event.code === 4400 || event.code === 4029)) {
        meta.lastError = '实时通道协议错误（' + event.code + '）';
        setTransport('rest');
        return;
      }
      scheduleReconnect();
    };
  }

  function send(payload) {
    if (!socket || socket.readyState !== 1) return false;
    try { socket.send(JSON.stringify(payload)); return true; } catch (error) { return false; }
  }

  function activeTopics() {
    const topics = ['status'];
    topicSubs.forEach(function (set, topic) {
      /* Only live topics may be pushed. Subscribing to a poll-only topic
       * (profiles, config) makes the server reject the whole frame with
       * "unknown topic", which drops every other topic in the same sync. */
      if (set.size && specs[topic] && specs[topic].live) topics.push(topic);
    });
    return topics.filter(function (topic, index, list) { return list.indexOf(topic) === index; });
  }

  function syncSubscriptions() {
    if (!meta.connected) return;
    send({ type: 'subscribe', topics: activeTopics() });
  }

  async function inflate(base64) {
    const binary = atob(base64);
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
    const decompressed = new Blob([bytes]).stream().pipeThrough(new DecompressionStream('gzip'));
    const text = await new Response(decompressed).text();
    return JSON.parse(text);
  }

  async function handleFrame(raw) {
    if (typeof raw !== 'string' || raw.length > 2 * 1024 * 1024) return;
    let envelope;
    try { envelope = JSON.parse(raw); } catch (error) { return; }
    if (envelope.encoding === 'gzip+base64') {
      if (!canGzip()) return;
      try { envelope.data = await inflate(envelope.data); } catch (error) { return; }
    }
    const type = envelope.type;
    const data = envelope.data || {};
    if (typeof envelope.sequence === 'number') meta.lastSequence = Math.max(meta.lastSequence, envelope.sequence);

    if (type === 'hello') {
      meta.protocol = data.protocol || PROTOCOL;
      meta.bootId = (data.server && data.server.bootId) || '';
      if (meta.protocol !== PROTOCOL) {
        meta.lastError = '协议版本不匹配';
        teardownSocket();
        setTransport('rest');
        return;
      }
      if (data.authRequired && cfg.secret) send({ type: 'auth', secret: cfg.secret });
      emit('meta');
      return;
    }
    if (type === 'auth.required') {
      if (cfg.secret) send({ type: 'auth', secret: cfg.secret });
      else {
        closedByAuth = true;
        teardownSocket();
        setTransport('rest');
        if (window.FlClashUI) window.FlClashUI.toast('后端要求 API 密钥', 'error');
      }
      return;
    }
    if (type === 'ping') {
      send({ type: 'pong', ts: Date.now() });
      if (Date.now() - lastAckAt > ACK_EVERY_MS) {
        lastAckAt = Date.now();
        send({ type: 'ack', sequence: meta.lastSequence });
      }
      return;
    }
    if (type === 'pong' || type === 'subscribed' || type === 'unsubscribed') return;
    if (type === 'resync') {
      // The following snapshot is authoritative; clear dedupe state so nothing
      // is dropped after the reset.
      topicSequence.clear();
      return;
    }
    if (type === 'protocol.error' || type === 'error') {
      meta.lastError = data.message || '实时通道错误';
      console.warn('[comm]', meta.lastError);
      return;
    }
    if (type === 'snapshot') {
      applySnapshot(data);
      return;
    }

    const topic = envelope.topic;
    if (!topic) return;
    // Sequence-based de-duplication per topic (reconnect may replay frames).
    const seq = envelope.topicSequence;
    if (typeof seq === 'number') {
      const last = topicSequence.get(topic) || 0;
      if (seq <= last) return;
      topicSequence.set(topic, seq);
    }

    switch (type) {
      case 'status': applyStatus(data); emit('status'); break;
      case 'traffic': applyTraffic(data); emit('status'); emit('traffic'); break;
      case 'proxies': state.proxies = data; emit('proxies'); break;
      case 'connections': state.connections = Array.isArray(data) ? data : []; emit('connections'); break;
      case 'logs.append': appendLogs(data.lines || []); emit('logs'); break;
      case 'task': upsertTask(data); emit('tasks'); break;
      case 'proxy.select': applyProxySelect(data); emit('proxies'); break;
      case 'proxy.delay': applyProxyDelay(data.name, data.value, data.tested); emit('proxies'); break;
      case 'proxy.delay.batch': applyProxyDelayBatch(data.delays || {}); emit('proxies'); break;
      case 'profiles.changed': state.profiles = Array.isArray(data) ? data : []; emit('profiles'); break;
      case 'system.config': refresh('config').catch(function () {}); break;
      case 'system.service': refresh('status').catch(function () {}); break;
      default: break;
    }
  }

  function applySnapshot(data) {
    if (data.status) applyStatus(data.status);
    if (data.proxies) state.proxies = data.proxies;
    if (Array.isArray(data.profiles)) state.profiles = data.profiles;
    if (Array.isArray(data.tasks)) { state.tasks = data.tasks; resolveTaskWaiters(); }
    if (data.capabilities) state.capabilities = data.capabilities;
    if (data.bootId) meta.bootId = data.bootId;
    if (typeof data.sequence === 'number') meta.lastSequence = Math.max(meta.lastSequence, data.sequence);
    topicSequence.clear();
    emit('status');
    emit('proxies');
    emit('profiles');
    emit('tasks');
    emit('meta');
    if (data.reason === 'gap' && window.FlClashUI) window.FlClashUI.toast('实时数据已重新同步', 'ok');
  }

  function applyTraffic(traffic) {
    if (!state.status) return;
    const merged = Object.assign({}, state.status.traffic || {}, traffic);
    state.status = Object.assign({}, state.status, { traffic: merged });
    if (traffic.history && Array.isArray(traffic.history.up)) {
      state.history.up = traffic.history.up.slice(-60);
      state.history.down = traffic.history.down.slice(-60);
    }
  }

  function appendLogs(lines) {
    if (!lines.length) return;
    state.logs = state.logs.concat(lines).slice(-300);
  }

  /* A task can finish before its waiter registers, or its completion can
   * arrive via a bulk payload (snapshot / REST poll) instead of a `task`
   * event. Every path that replaces state.tasks must settle pending waits. */
  function resolveTaskWaiters() {
    if (!taskWaiters.size) return;
    state.tasks.forEach(function (task) {
      const waiter = task && taskWaiters.get(task.id);
      if (waiter && (task.state === 'done' || task.state === 'failed')) {
        taskWaiters.delete(task.id);
        if (task.state === 'done') waiter.resolve(task);
        else waiter.reject(new Error(task.error || '任务失败'));
      }
    });
  }

  function upsertTask(task) {
    if (!task || !task.id) return;
    const list = state.tasks.slice();
    const index = list.findIndex(function (item) { return item.id === task.id; });
    if (index >= 0) list[index] = task; else list.unshift(task);
    state.tasks = list.slice(0, 64);
    const waiter = taskWaiters.get(task.id);
    if (waiter && (task.state === 'done' || task.state === 'failed')) {
      taskWaiters.delete(task.id);
      if (task.state === 'done') waiter.resolve(task);
      else waiter.reject(new Error(task.error || '任务失败'));
    }
  }

  function applyProxySelect(info) {
    if (!state.proxies || !info) return;
    const groups = (state.proxies.groups || []).map(function (group) {
      if (group.name === info.group || group.name === info.requestedGroup) {
        return Object.assign({}, group, { now: info.name });
      }
      return group;
    });
    state.proxies = Object.assign({}, state.proxies, { groups: groups });
  }

  function applyProxyDelay(name, value, tested) {
    if (!state.proxies || !state.proxies.nodes) return;
    const nodes = state.proxies.nodes.map(function (node) {
      if (node.name !== name) return node;
      return Object.assign({}, node, { delay: value, tested: tested !== false });
    });
    state.proxies = Object.assign({}, state.proxies, { nodes: nodes });
  }

  function applyProxyDelayBatch(delays) {
    if (!state.proxies || !state.proxies.nodes) return;
    const nodes = state.proxies.nodes.map(function (node) {
      if (!(node.name in delays)) return node;
      return Object.assign({}, node, { delay: Number(delays[node.name]) || 0, tested: true });
    });
    state.proxies = Object.assign({}, state.proxies, { nodes: nodes });
  }

  function startHeartbeat() {
    if (watchdog) window.clearInterval(watchdog);
    watchdog = window.setInterval(function () {
      if (!meta.connected) return;
      if (Date.now() - lastServerFrame > HEARTBEAT_TIMEOUT_MS) {
        meta.lastError = '实时通道心跳超时';
        teardownSocket();
        scheduleReconnect();
      }
    }, 5000);
    if (ackTimer) window.clearInterval(ackTimer);
    ackTimer = window.setInterval(function () {
      if (!meta.connected) return;
      if (Date.now() - lastAckAt > ACK_EVERY_MS) {
        lastAckAt = Date.now();
        send({ type: 'ack', sequence: meta.lastSequence });
      }
    }, ACK_EVERY_MS);
  }

  /* -------------------------------------------------------------- public */

  function watch(topic, handler) {
    if (!specs[topic]) throw new Error('unknown topic: ' + topic);
    const set = subscribers(topic);
    set.add(handler);
    if (meta.connected) syncSubscriptions();
    startPolling(topic);
    if (state[specs[topic].key] !== null && state[specs[topic].key] !== undefined) {
      try { handler(state[specs[topic].key], topic); } catch (error) { console.error(error); }
    }
    const current = state[specs[topic].key];
    if (!inflight.has(topic) && (current === null || current === undefined || (Array.isArray(current) && current.length === 0))) {
      refresh(topic).catch(function () {});
    }
    return function () {
      const current = topicSubs.get(topic);
      if (!current) return;
      current.delete(handler);
      if (current.size === 0) {
        stopPolling(topic);
        topicSubs.delete(topic);
        if (meta.connected) send({ type: 'unsubscribe', topics: [topic] });
      }
    };
  }

  function start() {
    if (cfg.mock) { setTransport('mock'); return; }
    connect();
    startHeartbeat();
    document.addEventListener('visibilitychange', function () {
      if (document.hidden) return;
      // A tab that slept through events re-syncs instead of trusting a gap.
      if (meta.connected) send({ type: 'sync' });
      topicSubs.forEach(function (set, topic) {
        if (set.size) refresh(topic).catch(function () {});
      });
    });
  }

  async function waitTask(id, timeoutMs) {
    if (!id) return null;
    const existing = state.tasks.find(function (task) { return task.id === id; });
    if (existing && (existing.state === 'done' || existing.state === 'failed')) {
      return existing.state === 'done' ? existing : Promise.reject(new Error(existing.error || '任务失败'));
    }
    return new Promise(function (resolve, reject) {
      // Subscribe to the tasks topic for the duration of the wait: the
      // calling view may not watch tasks itself, and a one-shot fetch can
      // land while the task is still running (fast tasks then never resolve).
      const unwatch = watch('tasks', function () {});
      const cleanup = function () { window.clearTimeout(timer); unwatch(); };
      const timer = window.setTimeout(function () {
        taskWaiters.delete(id);
        cleanup();
        reject(new Error('任务超时'));
      }, timeoutMs || 300000);
      taskWaiters.set(id, {
        resolve: function (task) { cleanup(); resolve(task); },
        reject: function (error) { cleanup(); reject(error); }
      });
      // Also fetch once in case the task finished before this waiter existed.
      refresh('tasks', { force: true }).catch(function () {});
    });
  }

  async function runTask(starter, options) {
    const reply = await starter();
    if (reply && reply.taskId) return waitTask(reply.taskId, options && options.timeout);
    return reply;
  }

  function onMeta(handler) {
    metaSubs.add(handler);
    try { handler(Object.assign({}, meta)); } catch (error) { console.error(error); }
    return function () { metaSubs.delete(handler); };
  }

  return {
    state: state,
    meta: meta,
    specs: specs,
    start: start,
    watch: watch,
    refresh: refresh,
    ensure: startPolling,
    subscribe: function (topic, handler) { return watch(topic, handler); },
    unsubscribe: function (topic) { stopPolling(topic); topicSubs.delete(topic); },
    send: send,
    sync: function () { send({ type: 'sync' }); },
    reconnect: function () { teardownSocket(); attempts = 0; closedByAuth = false; connect(); },
    waitTask: waitTask,
    runTask: runTask,
    onMeta: onMeta,
    close: function () { closedByAuth = true; teardownSocket(); },
    _applyStatus: applyStatus
  };
})();
