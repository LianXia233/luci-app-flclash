/* Demo backend used by ?mock=1 — mirrors the Go service's JSON contract. */
window.FlClashMock = (function () {
  'use strict';

  const KB = 1024;
  const MB = KB * 1024;
  const GB = MB * 1024;

  const state = {
    running: true,
    mode: 'rule',
    systemProxy: true,
    tun: false,
    allowLan: true,
    ipv6: true,
    mixedPort: 7890,
    socksPort: 7891,
    httpPort: 7892,
    logLevel: 'info',
    startedAt: Date.now() - 1000 * 60 * 48,
    upRate: 42 * KB,
    downRate: 1.4 * MB,
    upTotal: 440.5 * MB,
    downTotal: 5.3 * GB,
    history: []
  };

  for (let i = 0; i < 40; i += 1) {
    const wobble = 0.55 + 0.45 * Math.sin(i / 3.1);
    state.history.push({ up: 18 * KB * wobble, down: 900 * KB * wobble });
  }

  const profiles = [
    {
      id: 'p1', name: 'ONPIXEL', type: 'url',
      url: 'https://sub.example.com/api/v1/client/subscribe?token=demo',
      updatedAt: '刚刚', expireDays: 184, usedShare: 0.37,
      used: '1.3 GB', total: '600 GB', proxies: 128, groups: 4, rules: 921, active: true
    },
    {
      id: 'p2', name: '本地备份', type: 'file', url: '/etc/flclash/backup.yaml',
      updatedAt: '昨天', expireDays: 0, usedShare: 0,
      used: '0 B', total: '0 B', proxies: 42, groups: 3, rules: 318, active: false
    }
  ];

  const groups = [
    {
      name: '节点选择', type: 'select', now: '自动选择', timeout: 0,
      nodes: [
        { name: '香港 01 · IPLC', type: 'vmess', delay: 42, country: 'HK' },
        { name: '日本 02 · BGP', type: 'vless', delay: 68, country: 'JP' },
        { name: '新加坡 03 · 中转', type: 'trojan', delay: 91, country: 'SG' },
        { name: '美国 04 · 家宽', type: 'hysteria2', delay: 0, country: 'US' },
        { name: '自动选择', type: 'url-test', delay: 42, country: '' }
      ]
    },
    {
      name: '自动选择', type: 'url-test', now: '香港 01 · IPLC', timeout: 0,
      nodes: [
        { name: '香港 01 · IPLC', type: 'vmess', delay: 42, country: 'HK' },
        { name: '日本 02 · BGP', type: 'vless', delay: 68, country: 'JP' },
        { name: '新加坡 03 · 中转', type: 'trojan', delay: 91, country: 'SG' }
      ]
    },
    {
      name: '负载均衡', type: 'load-balance', now: '4 个节点', timeout: 1,
      nodes: [
        { name: '香港 01 · IPLC', type: 'vmess', delay: 42, country: 'HK' },
        { name: '日本 02 · BGP', type: 'vless', delay: 68, country: 'JP' },
        { name: '新加坡 03 · 中转', type: 'trojan', delay: 91, country: 'SG' },
        { name: '美国 04 · 家宽', type: 'hysteria2', delay: 0, country: 'US' }
      ]
    },
    {
      name: '故障转移', type: 'fallback', now: '美国 04 · 家宽', timeout: 2,
      nodes: [
        { name: '美国 04 · 家宽', type: 'hysteria2', delay: 176, country: 'US' },
        { name: '香港 01 · IPLC', type: 'vmess', delay: 42, country: 'HK' }
      ]
    }
  ];

  const services = [
    { label: 'Google', delay: 187, ok: true },
    { label: 'Cloudflare', delay: 112, ok: true },
    { label: 'GitHub', delay: 238, ok: true },
    { label: 'Telegram', delay: 0, ok: false }
  ];

  const hosts = [
    ['www.bilibili.com', 443, 'DIRECT', '国内直连'],
    ['api.github.com', 443, '节点选择', '规则匹配'],
    ['www.youtube.com', 443, '节点选择', 'GEOSITE'],
    ['api.openai.com', 443, '自动选择', 'DOMAIN-SUFFIX'],
    ['cdn.jsdelivr.net', 443, '负载均衡', 'GEOIP'],
    ['music.163.com', 443, 'DIRECT', '国内直连'],
    ['steamcdn-a.akamaihd.net', 443, '负载均衡', 'MATCH'],
    ['registry.npmjs.org', 443, '节点选择', 'DOMAIN-KEYWORD']
  ];

  let connections = [];
  let logs = [];
  let dnsQueries = [];

  function pick(list) { return list[Math.floor(Math.random() * list.length)]; }
  function randomIp() {
    return '104.' + (16 + Math.floor(Math.random() * 40)) + '.' + Math.floor(Math.random() * 255) + '.' + Math.floor(Math.random() * 255);
  }

  function seedConnection(index) {
    const host = hosts[index % hosts.length];
    const start = Date.now() - Math.floor(Math.random() * 600000);
    return {
      id: 'c' + start + '-' + index,
      host: host[0], port: host[1], rule: host[3], chain: host[2],
      source: '192.0.2.' + (2 + (index % 20)),
      up: Math.floor(Math.random() * 8 * MB),
      down: Math.floor(Math.random() * 60 * MB),
      startTime: start,
      network: 'tcp',
      process: pick(['chrome.exe', 'Clash', 'Telegram', 'curl', 'Steam'])
    };
  }

  for (let i = 0; i < 7; i += 1) connections.push(seedConnection(i));

  const logSeed = [
    ['info', 'Start initial configuration in progress'],
    ['info', 'GeoSite loaded: geosite.dat'],
    ['info', 'Rule set loaded: cn-domain (18432 rules)'],
    ['info', '[TCP] www.bilibili.com:443 --> DIRECT'],
    ['info', '[TCP] api.github.com:443 --> 香港 01 · IPLC'],
    ['warning', '延迟测试失败: 美国 04 · 家宽'],
    ['info', 'DNS query api.github.com resolved by dns.google'],
    ['info', '[UDP] 192.0.2.12:5353 --> dns.google'],
    ['error', 'proxy 美国 04 · 家宽: dial tcp timeout'],
    ['info', 'Rule set updated: geosite-cn']
  ];

  function seedLogs() {
    const now = Date.now();
    logs = logSeed.map(function (item, index) {
      return { time: now - (logSeed.length - index) * 7000, level: item[0], message: item[1] };
    });
  }

  function seedDns() {
    const now = Date.now();
    const names = ['www.bilibili.com', 'api.github.com', 'example.com', 'api.openai.com', 'cdn.jsdelivr.net', 'music.163.com'];
    dnsQueries = names.map(function (name, index) {
      return {
        time: now - index * 9000,
        name: name,
        type: index % 3 === 0 ? 'AAAA' : 'A',
        answer: index % 2 === 0 ? '198.18.0.' + (index + 2) : 'DIRECT',
        resolver: index % 2 === 0 ? 'dns.google' : 'system',
        elapsed: 8 + Math.floor(Math.random() * 60)
      };
    });
  }

  seedLogs();
  seedDns();

  function tick() {
    if (!state.running) {
      state.upRate = 0;
      state.downRate = 0;
    } else {
      state.upRate = Math.min(2.5 * MB, Math.max(2 * KB, state.upRate * (0.88 + Math.random() * 0.26)));
      state.downRate = Math.min(18 * MB, Math.max(24 * KB, state.downRate * (0.88 + Math.random() * 0.3)));
      if (Math.random() > 0.96) state.downRate = (1.2 + Math.random() * 2) * MB;
      state.upTotal += state.upRate;
      state.downTotal += state.downRate;
    }
    state.history.push({ up: state.upRate, down: state.downRate });
    if (state.history.length > 40) state.history.shift();

    if (logs.length) {
      const last = logs[logs.length - 1].time;
      if (Date.now() - last > 4200 && state.running) {
        const host = pick(hosts);
        logs.push({
          time: Date.now(),
          level: Math.random() > 0.9 ? 'warning' : 'info',
          message: '[TCP] ' + host[0] + ':' + host[1] + ' --> ' + host[2]
        });
        if (logs.length > 200) logs.shift();
      }
    }
    if (dnsQueries.length && Math.random() > 0.7) {
      const host = pick(hosts);
      dnsQueries.unshift({
        time: Date.now(), name: host[0], type: 'A',
        answer: '198.18.0.' + (2 + Math.floor(Math.random() * 200)),
        resolver: 'dns.google', elapsed: 8 + Math.floor(Math.random() * 60)
      });
      if (dnsQueries.length > 60) dnsQueries.pop();
    }
    if (state.running && Math.random() > 0.72) {
      connections.unshift(seedConnection(Math.floor(Math.random() * 100)));
      if (connections.length > 24) connections.pop();
    }
    connections.forEach(function (item) {
      if (state.running) {
        item.up += Math.floor(Math.random() * 4096);
        item.down += Math.floor(Math.random() * 32768);
      }
    });
  }

  window.setInterval(tick, 1000);

  function clone(value) { return JSON.parse(JSON.stringify(value)); }

  function uptime() { return state.running ? (Date.now() - state.startedAt) / 1000 : 0; }

  function traffic() {
    const peakUp = Math.max.apply(null, state.history.map(function (i) { return i.up; }));
    const peakDown = Math.max.apply(null, state.history.map(function (i) { return i.down; }));
    return {
      up: state.upRate, down: state.downRate,
      upRate: state.upRate, downRate: state.downRate,
      peakUp: peakUp, peakDown: peakDown,
      totalUp: state.upTotal, totalDown: state.downTotal,
      history: {
        up: state.history.map(function (i) { return i.up; }),
        down: state.history.map(function (i) { return i.down; })
      }
    };
  }

  function status() {
    const active = profiles.find(function (p) { return p.active; }) || profiles[0];
    return Promise.resolve(clone({
      running: state.running,
      version: '0.9.3',
      coreVersion: 'v1.19.33',
      uptime: uptime(),
      mode: state.mode,
      outboundIp: '203.0.113.62',
      ipv4: '203.0.113.62',
      ipv6: '2001:db8::62',
      countryCode: 'HK',
      organization: 'EXAMPLE NETWORK LTD',
      quality: { type: '商业', level: '质量优' },
      localIp: '192.0.2.177',
      mixedPort: state.mixedPort,
      socksPort: state.socksPort,
      httpPort: state.httpPort,
      systemProxy: state.systemProxy,
      tun: state.tun,
      allowLan: state.allowLan,
      ipv6Enabled: state.ipv6,
      logLevel: state.logLevel,
      traffic: traffic(),
      memory: { app: 61.4 * MB, core: 48.9 * MB },
      services: services,
      stats: { connections: connections.length, requests: connections.length * 3 + 12, dns: dnsQueries.length },
      profile: active,
      proxyGroups: {
        count: groups.length,
        timeout: groups.reduce(function (n, g) { return n + (g.timeout || 0); }, 0),
        groups: groups.map(function (g) { return { name: g.name, type: g.type, now: g.now, count: g.nodes.length }; })
      }
    }));
  }

  function nodeList() {
    const seen = {};
    const nodes = [];
    groups.forEach(function (group) {
      group.nodes.forEach(function (node) {
        if (seen[node.name]) { seen[node.name].groups.push(group.name); return; }
        const copy = Object.assign({}, node, { group: group.name, groups: [group.name] });
        seen[node.name] = copy;
        nodes.push(copy);
      });
    });
    return nodes;
  }

  return {
    status: status,
    memory: function () {
      return status().then(function (value) {
        return { rss: value.memory.app + value.memory.core, app: value.memory.app, core: value.memory.core };
      });
    },
    profiles: function () { return Promise.resolve(clone(profiles)); },
    proxies: function () {
      return Promise.resolve({
        groups: groups.map(function (g) { return Object.assign(clone(g), { count: g.nodes.length }); }),
        nodes: clone(nodeList())
      });
    },
    connections: function () {
      return Promise.resolve(clone(connections.map(function (item) {
        return Object.assign({}, item, { duration: (Date.now() - item.startTime) / 1000 });
      })));
    },
    dnsQueries: function () { return Promise.resolve(clone(dnsQueries)); },
    logs: function () { return Promise.resolve(clone(logs)); },
    config: function () {
      return Promise.resolve(clone({
        mode: state.mode, mixedPort: state.mixedPort, socksPort: state.socksPort,
        httpPort: state.httpPort, systemProxy: state.systemProxy, tun: state.tun,
        allowLan: state.allowLan, ipv6: state.ipv6, logLevel: state.logLevel
      }));
    },
    version: function () { return Promise.resolve({ version: '0.9.3', core: 'v1.19.33' }); },
    systemProxy: function () {
      return Promise.resolve({
        backend: 'flclash',
        mixed_port: 7892,
        clash_api_port: 9097,
        tun_name: '',
        routing_mode: 'rule',
        ipv6_support: false,
        running: true,
        read_only: true,
        source: 'mock'
      });
    },
    importProfile: function (payload) {
      const link = typeof payload === 'string' ? payload : (payload && payload.url) || '';
      const profile = {
        id: 'p' + Date.now(), name: link.split('/').pop() || '新订阅', type: 'url', url: link,
        updatedAt: '刚刚', expireDays: 0, usedShare: 0, used: '0 B', total: '0 B',
        proxies: 0, groups: 0, rules: 0, active: false
      };
      profiles.push(profile);
      return Promise.resolve(clone(profile));
    },
    applyProfile: function (id) {
      profiles.forEach(function (p) { p.active = p.id === id; });
      return Promise.resolve({ ok: true, id: id });
    },
    deleteProfile: function (id) {
      const index = profiles.findIndex(function (p) { return p.id === id; });
      if (index >= 0) profiles.splice(index, 1);
      return Promise.resolve({ ok: true, id: id });
    },
    selectProxy: function (payload) {
      const group = groups.find(function (g) { return g.name === (payload && payload.group); });
      if (group) group.now = payload.name;
      return Promise.resolve({ ok: true });
    },
    testDelay: function (payload) {
      const name = typeof payload === 'string' ? payload : (payload && payload.name) || '';
      const value = 24 + Math.floor(Math.random() * 180);
      groups.forEach(function (group) {
        group.nodes.forEach(function (node) {
          if (node.name === name) node.delay = value;
        });
      });
      return Promise.resolve({ name: name, value: value });
    },
    closeConnection: function (id) {
      connections = connections.filter(function (item) { return item.id !== id; });
      return Promise.resolve({ ok: true, id: id });
    },
    flushDns: function () {
      dnsQueries = [];
      return Promise.resolve({ ok: true });
    },
    saveConfig: function (patch) {
      if (patch && typeof patch === 'object') {
        if (patch.mode) state.mode = patch.mode;
        if (patch.mixedPort) state.mixedPort = Number(patch.mixedPort);
        if (patch.socksPort) state.socksPort = Number(patch.socksPort);
        if (patch.httpPort) state.httpPort = Number(patch.httpPort);
        if (typeof patch.systemProxy === 'boolean') state.systemProxy = patch.systemProxy;
        if (typeof patch.tun === 'boolean') state.tun = patch.tun;
        if (typeof patch.allowLan === 'boolean') state.allowLan = patch.allowLan;
        if (typeof patch.ipv6 === 'boolean') state.ipv6 = patch.ipv6;
        if (patch.logLevel) state.logLevel = patch.logLevel;
      }
      return Promise.resolve({ ok: true });
    },
    service: function (action) {
      if (action === 'start') { state.running = true; state.startedAt = Date.now(); }
      if (action === 'stop') state.running = false;
      if (action === 'restart') { state.running = true; state.startedAt = Date.now(); }
      return Promise.resolve({ ok: true, action: action || 'start' });
    }
  };
})();
