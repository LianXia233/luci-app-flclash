/* FlClash WebUI — shell, router and dashboard. */
(function () {
  'use strict';

  const api = window.FlClashAPI;
  const cfg = window.FlClashConfig;
  const ui = window.FlClashUI;
  const views = window.FlClashViews;
  const icon = ui.icon;

  const viewRoot = document.getElementById('view-root');
  const pageTitle = document.getElementById('page-title');
  const pageSub = document.getElementById('page-sub');
  const headerTraffic = document.getElementById('header-traffic');
  const sideStatus = document.getElementById('side-status');
  const sideStatusText = document.getElementById('side-status-text');
  const sideVersion = document.getElementById('side-version');

  const state = {
    status: null,
    proxies: null,
    profiles: null,
    connections: null,
    logs: null,
    dns: null,
    config: null,
    history: { up: [], down: [] },
    loaded: false,
    offline: false
  };

  const NAV = [
    { id: 'dashboard', label: '仪表盘', glyph: 'dashboard' },
    { id: 'proxies', label: '代理', glyph: 'proxies' },
    { id: 'profiles', label: '配置', glyph: 'profiles' },
    { id: 'requests', label: '请求', glyph: 'requests' },
    { id: 'tools', label: '工具', glyph: 'tools' }
  ];

  const META = {
    dashboard: '仪表盘',
    proxies: '代理',
    profiles: '配置',
    requests: '请求',
    tools: '工具',
    dns: 'DNS',
    logs: '日志',
    settings: '设置'
  };

  let current = 'dashboard';
  let mountToken = 0;
  let dashboardNode = null;

  /* ------------------------------------------------------------- utilities */

  function isMobile() { return window.innerWidth < 860; }

  function buildNav() {
    const side = document.getElementById('side-nav');
    const bottom = document.getElementById('bottom-nav');
    side.innerHTML = NAV.map(function (item) {
      return '<button class="side-item" data-view="' + item.id + '" type="button">' +
        '<span data-icon="' + item.glyph + '"></span><span>' + item.label + '</span></button>';
    }).join('');
    bottom.innerHTML = NAV.map(function (item) {
      return '<button class="bottom-item" data-view="' + item.id + '" type="button">' +
        '<span data-icon="' + item.glyph + '"></span><span>' + item.label + '</span></button>';
    }).join('');
    window.FlClashIcons.mount(side);
    window.FlClashIcons.mount(bottom);
    ui.$$('[data-view]').forEach(function (button) {
      button.addEventListener('click', function () {
        const next = button.getAttribute('data-view');
        if (window.location.hash === '#/' + next) navigate(next);
        else window.location.hash = '#/' + next;
      });
    });
  }

  function paintNav(name) {
    const root = ['dns', 'logs', 'settings'].indexOf(name) >= 0 ? 'tools' : name;
    ui.$$('.side-item,.bottom-item').forEach(function (button) {
      button.classList.toggle('is-active', button.getAttribute('data-view') === root);
    });
  }

  function setHeader(title, subtitle) {
    pageTitle.textContent = title;
    pageSub.textContent = subtitle || '';
  }

  function applyTheme(theme) {
    const next = theme || cfg.theme;
    cfg.theme = next;
    document.documentElement.setAttribute('data-theme', next);
    const meta = document.querySelector('meta[name="theme-color"]');
    if (meta) meta.setAttribute('content', next === 'dark' ? '#000000' : '#f2f2f7');
    try { window.localStorage.setItem('flclash.theme', next); } catch (error) { /* ignore */ }
    const toggle = document.getElementById('theme-toggle');
    toggle.innerHTML = icon('theme');
  }

  function setTheme(theme) {
    applyTheme(theme);
    if (current === 'settings') renderView('settings', true);
  }

  function subtitleFor(status) {
    if (!status) return '正在连接内核…';
    if (!status.running) return '内核未运行';
    return '运行中 · ' + ui.duration(status.uptime) + ' · ' + modeLabel(status.mode) + '模式';
  }

  function modeLabel(mode) {
    return mode === 'global' ? '全局' : mode === 'direct' ? '直连' : '规则';
  }

  function updateChrome(status) {
    state.status = status;
    state.offline = false;
    sideStatus.className = 'side-status is-ok';
    sideStatusText.textContent = status.running ? '内核运行中' : '内核已停止';
    if (!status.running) sideStatus.className = 'side-status is-off';
    sideVersion.textContent = 'v' + (status.version || '—') + ' · ' + (status.coreVersion || '');
    if (current === 'dashboard' || current === 'settings') setHeader(META[current], subtitleFor(status));
    const button = document.getElementById('core-status');
    button.className = 'round-action ' + (status.running ? 'is-ok' : 'is-warn');
    button.innerHTML = icon(status.running ? 'check' : 'refresh');
    button.setAttribute('aria-label', status.running ? '内核运行中' : '内核未运行');
    headerTraffic.innerHTML =
      '<span class="traffic-up"><i></i><b>' + ui.rate(status.traffic ? status.traffic.upRate : 0) + '</b></span>' +
      '<span class="traffic-down"><i></i><b>' + ui.rate(status.traffic ? status.traffic.downRate : 0) + '</b></span>';
  }

  function markOffline(error) {
    if (state.offline) return;
    state.offline = true;
    sideStatus.className = 'side-status is-off';
    sideStatusText.textContent = '未连接后端';
    ui.toast('后端不可达: ' + (error && error.message ? error.message : '网络错误'), 'error');
  }

  async function refreshStatus() {
    try {
      const status = await api.status();
      updateChrome(status);
      const traffic = status.traffic || {};
      const history = traffic.history;
      if (history && history.up && history.down) {
        state.history.up = history.up.slice(-40);
        state.history.down = history.down.slice(-40);
      } else {
        state.history.up.push(traffic.upRate || traffic.up || 0);
        state.history.down.push(traffic.downRate || traffic.down || 0);
        if (state.history.up.length > 40) state.history.up.shift();
        if (state.history.down.length > 40) state.history.down.shift();
      }
      if (current === 'dashboard' && dashboardNode) updateDashboard(status);
      if (current === 'settings') {
        const node = ui.$('[data-role="state"]', viewRoot);
        if (node) node.textContent = status.running ? '运行中' : '已停止';
      }
      return status;
    } catch (error) {
      markOffline(error);
      throw error;
    }
  }

  /* -------------------------------------------------------------- dashboard */

  function modeSegments(status) {
    const modes = [
      ['rule', '规则', 'split'],
      ['global', '全局', 'proxies'],
      ['direct', '直连', 'target']
    ];
    return '<div class="segmented" data-role="mode">' + modes.map(function (item) {
      return '<button class="segment' + (status.mode === item[0] ? ' is-active' : '') + '" data-mode="' + item[0] + '" type="button">' +
        icon(item[2]) + '<span>' + item[1] + '</span></button>';
    }).join('') + '</div>';
  }

  function statusCard(status) {
    const running = !!status.running;
    const quality = status.quality || {};
    return '<section class="card status-card span-2' + (running ? ' is-running' : '') + '" data-role="status">' +
      '<div class="status-inner">' +
        '<div class="status-main">' +
          '<div class="status-head">' +
            '<span class="glyph">' + icon('status') + '</span>' +
            '<span class="card-title">状态</span>' +
            '<span class="chip ' + (running ? 'is-ok' : '') + '" data-role="state-chip">' +
              (running ? '<i class="chip-dot"></i>运行中' : '未运行') + '</span>' +
          '</div>' +
          '<div class="status-ip-row">' +
            '<div class="country-badge" data-role="country">' + ui.esc(status.countryCode || '--') + '</div>' +
            '<div class="ip-stack">' +
              '<button class="ip-text" data-role="ip" type="button" title="复制出口 IP" style="text-align:left">' + ui.esc(status.outboundIp || '—') + '</button>' +
              '<div class="ip-chips">' +
                '<span class="chip">' + ui.esc(quality.type || '未知') + '</span>' +
                '<span class="chip ' + (quality.level === '质量优' ? 'is-ok' : '') + '">' + ui.esc(quality.level || '质量未知') + '</span>' +
              '</div>' +
            '</div>' +
          '</div>' +
          '<div class="status-cells">' +
            '<div class="status-cell"><div class="status-cell-label">运行时长</div>' +
              '<div class="status-cell-value" data-role="uptime">' + ui.duration(status.uptime) + '</div></div>' +
            '<div class="status-cell"><div class="status-cell-label">组织</div>' +
              '<div class="status-cell-value" data-role="org">' + ui.esc(status.organization || '—') + '</div></div>' +
          '</div>' +
        '</div>' +
        '<button class="start-toggle' + (running ? ' is-running' : '') + '" data-act="toggle" type="button" ' +
          'aria-label="' + (running ? '停止内核' : '启动内核') + '">' + icon(running ? 'pause' : 'play') + '</button>' +
      '</div>' +
    '</section>';
  }

  function speedCard(direction, status) {
    const up = direction === 'up';
    const traffic = status.traffic || {};
    const value = ui.splitBytes(up ? traffic.upRate : traffic.downRate);
    const peak = up ? traffic.peakUp : traffic.peakDown;
    const total = up ? traffic.totalUp : traffic.totalDown;
    return '<section class="card speed-card" data-role="speed-' + direction + '">' +
      '<div class="speed-inner">' +
        '<div class="speed-head">' +
          '<span class="glyph">' + icon(up ? 'arrowUp' : 'arrowDown') + '</span>' +
          '<span class="label">' + (up ? '上传' : '下载') + '</span>' +
          '<span class="peak" data-role="peak">峰值 ' + ui.rate(peak || 0) + '</span>' +
        '</div>' +
        '<div class="speed-figure"><span class="speed-value" data-role="value">' + value.value + '</span>' +
          '<span class="speed-unit" data-role="unit">' + value.unit + '/s</span></div>' +
        '<div class="speed-total" data-role="total">累计 ' + ui.bytes(total || 0) + '</div>' +
      '</div>' +
      '<div class="speed-chart ' + (up ? 'is-up' : 'is-down') + '" data-role="chart"></div>' +
    '</section>';
  }

  function mobileSpeedCard(status) {
    const traffic = status.traffic || {};
    function tile(up) {
      const value = ui.splitBytes(up ? traffic.upRate : traffic.downRate);
      return '<div class="node-card" style="background:var(--tone);cursor:default">' +
        '<div class="node-meta" style="gap:4px"><span class="glyph">' + icon(up ? 'arrowUp' : 'arrowDown') + '</span>' +
          '<b style="color:var(--muted);font-size:12px">' + (up ? '上传' : '下载') + '</b></div>' +
        '<div class="row" style="align-items:baseline;gap:4px"><span class="speed-value" style="font-size:26px" data-role="m-' + (up ? 'up' : 'down') + '">' + value.value + '</span>' +
          '<span class="speed-unit" style="font-size:12px">' + value.unit + '/s</span></div>' +
        '<div class="speed-total" style="margin:0">累计 ' + ui.bytes(up ? traffic.totalUp : traffic.totalDown) + '</div>' +
        '<div class="speed-chart ' + (up ? 'is-up' : 'is-down') + '" style="position:relative;left:0;right:0;bottom:0;height:52px;margin:2px -14px -10px" data-role="m-chart-' + (up ? 'up' : 'down') + '"></div>' +
      '</div>';
    }
    return '<section class="card">' +
      '<div class="card-head"><span class="glyph">' + icon('speed') + '</span><span class="card-title">网络速度</span></div>' +
      '<div style="display:grid;grid-template-columns:1fr 1fr;gap:10px">' + tile(true) + tile(false) + '</div>' +
    '</section>';
  }

  function modeBar(status) {
    return '<section class="card mode-bar span-2" data-role="mode-bar">' +
      '<span class="glyph">' + icon('split') + '</span>' +
      '<span class="mode-label">出站模式</span>' +
      modeSegments(status) +
    '</section>';
  }

  function takeoverTile(kind, status) {
    const system = kind === 'systemProxy';
    const on = system ? !!status.systemProxy : !!status.tun;
    return '<section class="card takeover-tile" data-role="takeover-' + kind + '">' +
      '<span class="tile-icon' + (on ? ' is-on' : '') + '" data-role="icon">' + icon(system ? 'shuffle' : 'vpn') + '</span>' +
      '<span class="tile-copy"><span class="tile-title">' + (system ? '系统代理' : '虚拟网卡') + '</span>' +
        '<span class="tile-sub">' + (system ? '端口 ' + (status.mixedPort || '—') : (on ? '已接管全局流量' : '未启用')) + '</span></span>' +
      ui.switchHtml(kind, on, system ? '系统代理' : '虚拟网卡') +
    '</section>';
  }

  function intranetCard(status) {
    const on = !!status.allowLan;
    return '<section class="card intranet-card" data-role="intranet">' +
      ui.head('内网 IP', {
        glyph: 'devices',
        end: '<button class="icon-button" data-act="copy-ip" type="button" aria-label="复制内网 IP">' + icon('copy') + '</button>'
      }) +
      '<div class="ip-figure" data-role="lan-ip">' + ui.esc(status.localIp || '—') + '</div>' +
      '<div class="sub-switch">' +
        '<span class="tile-icon' + (on ? ' is-on' : '') + '">' + icon('activity') + '</span>' +
        '<span class="tile-copy"><span class="tile-title">局域网代理</span>' +
          '<span class="tile-sub">' + (on ? '端口 ' + (status.mixedPort || '—') : '已关闭') + '</span></span>' +
        ui.switchHtml('allowLan', on, '局域网代理') +
      '</div>' +
    '</section>';
  }

  function profileCard(status) {
    const profile = status.profile || {};
    const share = Number(profile.usedShare) || 0;
    return '<section class="card is-tappable" data-role="profile" data-go="profiles">' +
      ui.head('配置', {
        glyph: 'profiles',
        chevron: true,
        end: '<span class="chip">' + ui.esc(profile.proxies || 0) + ' 节点</span>'
      }) +
      '<div class="profile-name">' + ui.esc(profile.name || '未选择配置') + '</div>' +
      '<div class="profile-meta"><span>更新于 ' + ui.esc(profile.updatedAt || '—') + '</span>' +
        (profile.expireDays ? '<span>剩余 ' + ui.esc(profile.expireDays) + ' 天</span>' : '') + '</div>' +
      '<div class="profile-usage">' +
        '<div class="usage-row"><strong>' + Math.round(share * 100) + '% 已用</strong>' +
          '<span>' + ui.esc(profile.used || '0 B') + ' / ' + ui.esc(profile.total || '0 B') + '</span></div>' +
        '<svg class="wave" viewBox="0 0 300 12" preserveAspectRatio="none">' + ui.wave(share, 300, 12) + '</svg>' +
      '</div>' +
    '</section>';
  }

  function serviceCard(status) {
    const services = status.services || [];
    return '<section class="card is-tappable" data-role="service" data-go="requests">' +
      ui.head('服务状态', { glyph: 'verified', chevron: true }) +
      '<div class="service-list" data-role="service-list">' + services.map(function (item) {
        return '<div class="service-row"><span class="service-icon">' + icon('proxies') + '</span>' +
          '<span class="tile-copy"><span class="tile-title">' + ui.esc(item.label) + '</span></span>' +
          '<span class="service-value' + (item.ok ? '' : ' is-bad') + '">' +
          (item.delay ? ui.esc(item.delay) + ' ms' : '超时') + '</span></div>';
      }).join('') + '</div>' +
    '</section>';
  }

  function memoryCard(status) {
    const memory = status.memory || { app: 0, core: 0 };
    const total = (memory.app || 0) + (memory.core || 0);
    const appShare = total ? (memory.app / total) * 100 : 50;
    return '<section class="card is-tappable" data-role="memory">' +
      ui.head('内存信息', { glyph: 'memory', chevron: true }) +
      '<div class="memory-value" data-role="memory-total">' + ui.bytes(total) + '</div>' +
      '<div class="memory-bar"><span class="memory-seg app" data-role="mem-app" style="width:' + appShare + '%"></span>' +
        '<span class="memory-seg core" data-role="mem-core" style="width:' + (100 - appShare) + '%"></span></div>' +
      '<div class="memory-legend">' +
        '<span class="legend-item"><i class="legend-dot"></i>应用 ' + ui.bytes(memory.app || 0) + '</span>' +
        '<span class="legend-item"><i class="legend-dot core"></i>内核 ' + ui.bytes(memory.core || 0) + '</span>' +
      '</div>' +
    '</section>';
  }

  function connectionStatsCard(status) {
    const stats = status.stats || {};
    const groups = status.proxyGroups || {};
    return '<section class="card" data-role="summaries">' +
      '<div class="stat-pairs">' +
        '<button class="stat-pill" data-go="requests" type="button"><span class="stat-count" data-role="conn-count">' + ui.count(stats.connections || 0) + '</span>' +
          '<span class="stat-label">实时连接</span></button>' +
        '<button class="stat-pill" data-go="proxies" type="button"><span class="stat-count" data-role="group-count">' + ui.count(groups.count || 0) + '</span>' +
          '<span class="stat-label">策略组</span></button>' +
      '</div>' +
    '</section>';
  }

  function connectionsCard(status) {
    const stats = status.stats || {};
    return '<section class="card is-flush" data-role="connections">' +
      '<div class="card-head" style="padding-right:22px"><span class="glyph">' + icon('requests') + '</span>' +
        '<span class="card-title">连接</span>' +
        '<span class="card-end"><span class="chip is-primary">' + ui.count(stats.connections || 0) + ' 实时</span>' +
          '<span class="chevron">' + icon('chevron') + '</span></span></div>' +
      '<div class="table-wrap" data-role="connections-table">' + ui.empty('暂无连接', 'requests') + '</div>' +
    '</section>';
  }

  function groupsCard(status) {
    const info = status.proxyGroups || { groups: [] };
    return '<section class="card" data-role="groups">' +
      ui.head('策略组', {
        glyph: 'layers',
        chevron: true,
        end: '<span class="chip">' + (info.count || 0) + '</span>' +
          (info.timeout ? '<span class="chip is-warn">' + info.timeout + ' 超时</span>' : '')
      }) +
      '<div class="group-list" data-role="group-list">' + (info.groups || []).map(function (group) {
        return '<div class="group-row"><span class="group-icon">' + icon('proxies') + '</span>' +
          '<span class="group-copy"><span class="group-name">' + ui.esc(group.name) + '</span>' +
            '<span class="group-now">' + ui.esc(group.now || '—') + '</span></span>' +
          '<span class="chip">' + ui.esc(group.count || 0) + '</span></div>';
      }).join('') + '</div>' +
    '</section>';
  }

  function renderDashboardShell(status) {
    const mobile = isMobile();
    const node = document.createElement('div');
    if (mobile) {
      node.className = 'dashboard is-mobile view';
      node.innerHTML = [
        statusCard(status),
        mobileSpeedCard(status),
        modeBar(status),
        takeoverTile('systemProxy', status),
        takeoverTile('tun', status),
        intranetCard(status),
        profileCard(status),
        serviceCard(status),
        memoryCard(status),
        connectionStatsCard(status),
        groupsCard(status)
      ].join('');
    } else {
      node.className = 'dashboard view';
      node.innerHTML =
        '<div class="dash-row">' + statusCard(status) + speedCard('up', status) + speedCard('down', status) + '</div>' +
        '<div class="dash-row is-control">' + modeBar(status) + takeoverTile('systemProxy', status) + takeoverTile('tun', status) + '</div>' +
        '<div class="dash-row is-mid">' + intranetCard(status) + profileCard(status) + serviceCard(status) + memoryCard(status) + '</div>' +
        '<div class="dash-row is-tables">' + connectionsCard(status) + groupsCard(status) + '</div>';
    }
    return node;
  }

  function bindDashboard(node) {
    ui.$$('[data-mode]', node).forEach(function (button) {
      button.addEventListener('click', function () {
        const mode = button.getAttribute('data-mode');
        if (state.status && state.status.mode === mode) return;
        ui.$$('[data-mode]', node).forEach(function (item) { item.classList.toggle('is-active', item === button); });
        api.saveConfig({ mode: mode })
          .then(function () { ui.toast('已切换到' + modeLabel(mode) + '模式', 'ok'); return refreshStatus(); })
          .catch(function (error) { ui.toast('切换失败: ' + error.message, 'error'); });
      });
    });

    ui.$$('[data-switch]', node).forEach(function (button) {
      button.addEventListener('click', function () {
        const key = button.getAttribute('data-switch');
        const on = !button.classList.contains('is-on');
        button.classList.toggle('is-on', on);
        button.setAttribute('aria-checked', String(on));
        const patch = {};
        patch[key] = on;
        api.saveConfig(patch)
          .then(function () { return refreshStatus(); })
          .catch(function (error) {
            button.classList.toggle('is-on', !on);
            ui.toast('设置失败: ' + error.message, 'error');
          });
      });
    });

    const toggle = ui.$('[data-act="toggle"]', node);
    if (toggle) {
      toggle.addEventListener('click', function () {
        const running = !!(state.status && state.status.running);
        toggle.disabled = true;
        api.service(running ? 'stop' : 'start')
          .then(function () { ui.toast(running ? '内核已停止' : '内核已启动', 'ok'); return refreshStatus(); })
          .catch(function (error) { ui.toast('操作失败: ' + error.message, 'error'); })
          .then(function () { toggle.disabled = false; });
      });
    }

    const ipButton = ui.$('[data-role="ip"]', node);
    if (ipButton) {
      ipButton.addEventListener('click', function () {
        const text = ipButton.textContent.trim();
        copy(text);
      });
    }
    const copyButton = ui.$('[data-act="copy-ip"]', node);
    if (copyButton) {
      copyButton.addEventListener('click', function () {
        copy((state.status && state.status.localIp) || '');
      });
    }

    const memory = ui.$('[data-role="memory"]', node);
    if (memory) {
      ui.pressable(memory, function () {
        const data = (state.status && state.status.memory) || { app: 0, core: 0 };
        ui.openSheet('内存信息',
          '<div class="kv"><span class="k">应用</span><span class="v">' + ui.bytes(data.app || 0) + '</span></div>' +
          '<div class="kv"><span class="k">内核</span><span class="v">' + ui.bytes(data.core || 0) + '</span></div>' +
          '<div class="kv"><span class="k">合计</span><span class="v">' + ui.bytes((data.app || 0) + (data.core || 0)) + '</span></div>' +
          '<p class="sheet-note">内存数据由后端上报；WebUI 仅展示，不参与采样。</p>');
      });
    }

    ui.$$('[data-go]', node).forEach(function (item) {
      ui.pressable(item, function () { navigate(item.getAttribute('data-go')); });
    });

    const connections = ui.$('[data-role="connections"]', node);
    if (connections) {
      ui.pressable(connections, function () { navigate('requests'); });
    }
  }

  function copy(text) {
    if (!text) return;
    const done = function () { ui.toast('已复制 ' + text, 'ok'); };
    if (navigator.clipboard && window.isSecureContext) {
      navigator.clipboard.writeText(text).then(done).catch(function () { fallbackCopy(text, done); });
    } else {
      fallbackCopy(text, done);
    }
  }

  function fallbackCopy(text, done) {
    const area = document.createElement('textarea');
    area.value = text;
    area.style.position = 'fixed';
    area.style.opacity = '0';
    document.body.appendChild(area);
    area.select();
    try { document.execCommand('copy'); done(); } catch (error) { ui.toast('复制失败', 'error'); }
    area.remove();
  }

  function updateDashboard(status) {
    const node = dashboardNode;
    if (!node) return;
    const running = !!status.running;

    const card = ui.$('[data-role="status"]', node);
    if (card) card.classList.toggle('is-running', running);

    const chip = ui.$('[data-role="state-chip"]', node);
    if (chip) {
      chip.className = 'chip ' + (running ? 'is-ok' : '');
      chip.innerHTML = running ? '<i class="chip-dot"></i>运行中' : '未运行';
    }

    const country = ui.$('[data-role="country"]', node);
    if (country) country.textContent = status.countryCode || '--';

    const ip = ui.$('[data-role="ip"]', node);
    if (ip && ip.textContent !== (status.outboundIp || '—')) ip.textContent = status.outboundIp || '—';

    const uptime = ui.$('[data-role="uptime"]', node);
    if (uptime) uptime.textContent = ui.duration(status.uptime);

    const org = ui.$('[data-role="org"]', node);
    if (org) org.textContent = status.organization || '—';

    const toggle = ui.$('[data-act="toggle"]', node);
    if (toggle) {
      toggle.classList.toggle('is-running', running);
      toggle.innerHTML = icon(running ? 'pause' : 'play');
      toggle.setAttribute('aria-label', running ? '停止内核' : '启动内核');
    }

    const traffic = status.traffic || {};
    [['up', traffic.upRate, traffic.totalUp, traffic.peakUp], ['down', traffic.downRate, traffic.totalDown, traffic.peakDown]].forEach(function (row) {
      const dir = row[0];
      const value = ui.splitBytes(row[1] || 0);
      const valueNode = ui.$('[data-role="speed-' + dir + '"] [data-role="value"]', node);
      const unitNode = ui.$('[data-role="speed-' + dir + '"] [data-role="unit"]', node);
      const totalNode = ui.$('[data-role="speed-' + dir + '"] [data-role="total"]', node);
      const peakNode = ui.$('[data-role="speed-' + dir + '"] [data-role="peak"]', node);
      if (valueNode) {
        ui.countUp(valueNode, Number(value.value), function (v) { return v.toFixed(1); });
        if (unitNode) unitNode.textContent = value.unit + '/s';
      }
      if (totalNode) totalNode.textContent = '累计 ' + ui.bytes(row[2] || 0);
      if (peakNode) peakNode.textContent = '峰值 ' + ui.rate(row[3] || 0);
      const chart = ui.$('[data-role="speed-' + dir + '"] [data-role="chart"]', node);
      if (chart) ui.renderChart(chart, dir === 'up' ? state.history.up : state.history.down, { minScale: 8192 });
      const mobileValue = ui.$('[data-role="m-' + dir + '"]', node);
      if (mobileValue) mobileValue.textContent = value.value;
      const mobileChart = ui.$('[data-role="m-chart-' + dir + '"]', node);
      if (mobileChart) ui.renderChart(mobileChart, dir === 'up' ? state.history.up : state.history.down, { height: 52, minScale: 8192 });
    });

    const modeBarNode = ui.$('[data-role="mode"]', node);
    if (modeBarNode) {
      ui.$$('[data-mode]', modeBarNode).forEach(function (button) {
        button.classList.toggle('is-active', button.getAttribute('data-mode') === status.mode);
      });
    }

    ['systemProxy', 'tun'].forEach(function (kind) {
      const tile = ui.$('[data-role="takeover-' + kind + '"]', node);
      if (!tile) return;
      const on = !!status[kind];
      const sw = ui.$('[data-switch]', tile);
      if (sw) { sw.classList.toggle('is-on', on); sw.setAttribute('aria-checked', String(on)); }
      const iconNode = ui.$('[data-role="icon"]', tile);
      if (iconNode) iconNode.classList.toggle('is-on', on);
      const sub = ui.$('.tile-sub', tile);
      if (sub && kind === 'tun') sub.textContent = on ? '已接管全局流量' : '未启用';
    });

    const lan = ui.$('[data-role="lan-ip"]', node);
    if (lan) lan.textContent = status.localIp || '—';

    const profile = ui.$('[data-role="profile"]', node);
    if (profile && status.profile) {
      const name = ui.$('.profile-name', profile);
      if (name) name.textContent = status.profile.name || '未选择配置';
      const wave = ui.$('.wave', profile);
      if (wave) wave.innerHTML = ui.wave(Number(status.profile.usedShare) || 0, 300, 12);
    }

    const serviceList = ui.$('[data-role="service-list"]', node);
    if (serviceList && status.services) {
      serviceList.innerHTML = status.services.map(function (item) {
        return '<div class="service-row"><span class="service-icon">' + icon('proxies') + '</span>' +
          '<span class="tile-copy"><span class="tile-title">' + ui.esc(item.label) + '</span></span>' +
          '<span class="service-value' + (item.ok ? '' : ' is-bad') + '">' +
          (item.delay ? ui.esc(item.delay) + ' ms' : '超时') + '</span></div>';
      }).join('');
    }

    const memory = status.memory || { app: 0, core: 0 };
    const total = (memory.app || 0) + (memory.core || 0);
    const totalNode = ui.$('[data-role="memory-total"]', node);
    if (totalNode) totalNode.textContent = ui.bytes(total);
    const appBar = ui.$('[data-role="mem-app"]', node);
    const coreBar = ui.$('[data-role="mem-core"]', node);
    if (appBar) appBar.style.width = (total ? (memory.app / total) * 100 : 50) + '%';
    if (coreBar) coreBar.style.width = (total ? (memory.core / total) * 100 : 50) + '%';

    const connCount = ui.$('[data-role="conn-count"]', node);
    if (connCount) connCount.textContent = ui.count((status.stats || {}).connections || 0);
    const groupCount = ui.$('[data-role="group-count"]', node);
    if (groupCount) groupCount.textContent = ui.count((status.proxyGroups || {}).count || 0);

    const groupList = ui.$('[data-role="group-list"]', node);
    if (groupList && status.proxyGroups) {
      groupList.innerHTML = (status.proxyGroups.groups || []).map(function (group) {
        return '<div class="group-row"><span class="group-icon">' + icon('proxies') + '</span>' +
          '<span class="group-copy"><span class="group-name">' + ui.esc(group.name) + '</span>' +
          '<span class="group-now">' + ui.esc(group.now || '—') + '</span></span>' +
          '<span class="chip">' + ui.esc(group.count || 0) + '</span></div>';
      }).join('');
    }

    updateConnectionsTable(status);
  }

  function updateConnectionsTable(status) {
    const wrap = ui.$('[data-role="connections-table"]', dashboardNode);
    if (!wrap) return;
    const list = state.connections;
    if (!list || !list.length) {
      wrap.innerHTML = ui.empty('暂无连接', 'requests');
      return;
    }
    wrap.innerHTML = '<table class="data"><thead><tr><th>主机</th><th>链路</th><th>上传</th><th>下载</th></tr></thead><tbody>' +
      list.slice(0, 8).map(function (item) {
        return '<tr><td>' + ui.esc(item.host) + '</td><td class="is-muted">' + ui.esc(item.chain || '-') + '</td>' +
          '<td class="num">' + ui.bytes(item.up) + '</td><td class="num">' + ui.bytes(item.down) + '</td></tr>';
      }).join('') + '</tbody></table>';
  }

  async function renderDashboard() {
    const token = ++mountToken;
    dashboardNode = null;
    let status = state.status;
    try {
      status = await api.status();
      updateChrome(status);
      if (token !== mountToken) return;
      if (!state.connections) {
        api.connections().then(function (list) { state.connections = list; if (token === mountToken) updateConnectionsTable(status); }).catch(function () {});
      }
    } catch (error) {
      markOffline(error);
      if (token !== mountToken) return;
    }
    if (!status) {
      viewRoot.innerHTML = '<div class="view">' + ui.empty('无法获取内核状态', 'info') + '</div>';
      return;
    }
    dashboardNode = renderDashboardShell(status);
    viewRoot.innerHTML = '';
    viewRoot.appendChild(dashboardNode);
    bindDashboard(dashboardNode);
    updateDashboard(status);

    ui.every('status', function () { refreshStatus().catch(function () {}); }, cfg.pollInterval);
    ui.every('connections', function () {
      api.connections().then(function (list) {
        state.connections = list;
        updateConnectionsTable(state.status);
        const chip = ui.$('[data-role="summaries"] [data-role="conn-count"]');
        if (chip) chip.textContent = ui.count(list.length);
      }).catch(function () {});
    }, 3000);
  }

  /* ----------------------------------------------------------------- router */

  const ctx = {
    api: api,
    state: state,
    navigate: function (name) { window.location.hash = '#/' + name; },
    refreshStatus: refreshStatus,
    reloadProfiles: async function () {
      state.profiles = await api.profiles();
      if (ctx.onProfiles) ctx.onProfiles();
      if (current === 'dashboard' && dashboardNode) updateDashboard(state.status || {});
    },
    reloadProxies: async function () {
      state.proxies = await api.proxies();
      if (ctx.onProxies) ctx.onProxies();
    },
    reloadConnections: async function () {
      state.connections = await api.connections();
      if (ctx.onConnections) ctx.onConnections();
    },
    setTheme: setTheme
  };

  const renderers = {
    dashboard: renderDashboard,
    proxies: async function () {
      try {
        state.proxies = await api.proxies();
      } catch (error) { markOffline(error); }
      if (!state.proxies) { viewRoot.innerHTML = '<div class="view">' + ui.empty('无法获取节点', 'proxies') + '</div>'; return; }
      const node = views.proxies(ctx);
      viewRoot.innerHTML = '';
      viewRoot.appendChild(node);
      ui.every('proxies', function () {
        api.proxies().then(function (data) {
          state.proxies = data;
          if (ctx.onProxies) ctx.onProxies();
        }).catch(function () {});
      }, 6000);
    },
    profiles: async function () {
      try {
        state.profiles = await api.profiles();
      } catch (error) { markOffline(error); }
      const node = views.profiles(ctx);
      viewRoot.innerHTML = '';
      viewRoot.appendChild(node);
      ui.every('profiles', function () {
        api.profiles().then(function (data) { state.profiles = data; if (ctx.onProfiles) ctx.onProfiles(); }).catch(function () {});
      }, 15000);
    },
    requests: async function () {
      if (!state.connections) {
        try { state.connections = await api.connections(); } catch (error) { markOffline(error); }
      }
      const node = views.requests(ctx);
      viewRoot.innerHTML = '';
      viewRoot.appendChild(node);
      ui.every('connections', function () {
        api.connections().then(function (list) {
          state.connections = list;
          if (ctx.onConnections) ctx.onConnections();
        }).catch(function () {});
      }, 3000);
    },
    logs: async function () {
      try { state.logs = await api.logs(); } catch (error) { markOffline(error); }
      const node = views.logs(ctx);
      viewRoot.innerHTML = '';
      viewRoot.appendChild(node);
      ui.every('logs', function () {
        api.logs().then(function (data) { state.logs = data; if (ctx.onLogs) ctx.onLogs(); }).catch(function () {});
      }, 2500);
    },
    dns: async function () {
      try { state.dns = await api.dnsQueries(); } catch (error) { markOffline(error); }
      const node = views.dns(ctx);
      viewRoot.innerHTML = '';
      viewRoot.appendChild(node);
      ui.every('dns', function () {
        api.dnsQueries().then(function (data) { state.dns = data; if (ctx.onDns) ctx.onDns(); }).catch(function () {});
      }, 5000);
    },
    tools: async function () {
      const node = views.tools(ctx);
      viewRoot.innerHTML = '';
      viewRoot.appendChild(node);
    },
    settings: async function () {
      try { state.config = await api.config(); } catch (error) { /* settings tolerates missing config */ }
      const node = views.settings(ctx);
      viewRoot.innerHTML = '';
      viewRoot.appendChild(node);
    }
  };

  const SUBTITLE = {
    proxies: '选择策略组与节点',
    profiles: '订阅与配置文件',
    requests: '实时连接与流量',
    logs: '内核运行日志',
    dns: '解析记录与缓存',
    tools: '诊断与维护',
    settings: '网络、内核与外观'
  };

  async function renderView(name, force) {
    if (!renderers[name]) name = 'dashboard';
    if (!force && current === name && name !== 'dashboard') return;
    current = name;
    ui.stopAll();
    paintNav(name);
    ctx.onProfiles = null;
    ctx.onProxies = null;
    ctx.onConnections = null;
    ctx.onLogs = null;
    ctx.onDns = null;
    setHeader(META[name] || '仪表盘', name === 'dashboard' ? subtitleFor(state.status) : (SUBTITLE[name] || ''));
    viewRoot.innerHTML = '<div class="view">' + skeleton() + '</div>';
    try {
      await renderers[name]();
    } catch (error) {
      viewRoot.innerHTML = '<div class="view">' + ui.empty('页面加载失败: ' + (error.message || error), 'info') + '</div>';
    }
  }

  function skeleton() {
    return '<div class="dashboard">' +
      '<div class="skeleton" style="height:208px;border-radius:32px"></div>' +
      '<div class="skeleton" style="height:176px;border-radius:32px"></div>' +
      '<div class="skeleton" style="height:176px;border-radius:32px"></div>' +
      '</div>';
  }

  function navigate(name) {
    if (!renderers[name]) name = 'dashboard';
    if (window.location.hash !== '#/' + name) {
      window.location.hash = '#/' + name;
      return;
    }
    renderView(name, true);
  }

  /* ------------------------------------------------------------------- init */

  function bindChrome() {
    document.getElementById('theme-toggle').addEventListener('click', function () {
      setTheme(document.documentElement.getAttribute('data-theme') === 'dark' ? 'light' : 'dark');
    });

    document.getElementById('core-status').addEventListener('click', function () {
      const button = this;
      button.classList.add('is-busy');
      api.service('restart')
        .then(function () { ui.toast('内核已重启', 'ok'); return refreshStatus(); })
        .catch(function (error) { ui.toast('重启失败: ' + error.message, 'error'); })
        .then(function () { button.classList.remove('is-busy'); });
    });

    document.getElementById('sheet-close').addEventListener('click', ui.closeSheet);
    document.getElementById('sheet-backdrop').addEventListener('click', ui.closeSheet);
    document.addEventListener('keydown', function (event) {
      if (event.key === 'Escape') ui.closeSheet();
    });

    let lastMobile = isMobile();
    window.addEventListener('resize', ui.debounce(function () {
      const mobile = isMobile();
      if (mobile !== lastMobile) {
        lastMobile = mobile;
        if (current === 'dashboard') renderDashboard();
      }
    }, 200));

    window.addEventListener('hashchange', function () {
      const name = window.location.hash.replace(/^#\/?/, '') || 'dashboard';
      if (name !== current || name === 'dashboard') renderView(name, true);
    });
  }

  async function init() {
    buildNav();
    applyTheme();
    bindChrome();
    const initial = window.location.hash.replace(/^#\/?/, '') || 'dashboard';
    try {
      const status = await api.status();
      updateChrome(status);
    } catch (error) {
      markOffline(error);
    }
    await renderView(initial === current ? 'dashboard' : initial, true);
    const splash = document.getElementById('boot-splash');
    if (splash) {
      splash.classList.add('is-hidden');
      window.setTimeout(function () { splash.remove(); }, 400);
    }
    ui.every('status', function () { refreshStatus().catch(function () {}); }, cfg.pollInterval);
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }
})();
