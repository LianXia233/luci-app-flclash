/* FlClash WebUI — shell, router and dashboard (aligned with the desktop
 * client: status card with flower FAB, speed cards with peak/total/chart,
 * outbound-mode segments, takeover tiles, profile/service/memory cards,
 * live connections table and proxy groups). */
(function () {
  'use strict';

  const api = window.FlClashAPI;
  const cfg = window.FlClashConfig;
  const ui = window.FlClashUI;
  const views = window.FlClashViews;
  const icon = window.FlClashIcons.svg;
  const flower = window.FlClashIcons.flower;

  const viewRoot = document.getElementById('view-root');
  const pageTitle = document.getElementById('page-title');
  const pageSub = document.getElementById('page-sub');
  const headerTraffic = document.getElementById('header-traffic');
  const miniUp = document.getElementById('mini-up');
  const miniDown = document.getElementById('mini-down');

  const state = {
    status: null,
    proxies: null,
    profiles: null,
    connections: null,
    logs: null,
    rules: null,
    config: null,
    history: { up: [], down: [] },
    offline: false
  };

  const NAV = [
    { id: 'dashboard', label: '仪表盘', glyph: 'dashboard' },
    { id: 'proxies', label: '代理', glyph: 'proxies' },
    { id: 'profiles', label: '配置', glyph: 'profiles' },
    { id: 'requests', label: '连接', glyph: 'requests' },
    { id: 'tools', label: '工具', glyph: 'tools' }
  ];

  const META = {
    dashboard: '仪表盘', proxies: '代理', profiles: '配置', requests: '连接',
    tools: '工具', dns: 'DNS 测试', logs: '日志', rules: '规则', settings: '设置'
  };

  let current = 'dashboard';
  let mountToken = 0;
  let dashboardNode = null;

  /* ------------------------------------------------------------- utilities */

  function isMobile() { return window.innerWidth < 860; }

  function buildNav() {
    const side = document.getElementById('rail-nav');
    const bottom = document.getElementById('bottom-nav');
    side.innerHTML = NAV.map(function (item) {
      return '<button class="rail-item" data-view="' + item.id + '" type="button" title="' + item.label + '">' +
        '<span class="rail-glyph" data-icon="' + item.glyph + '"></span><span class="rail-label">' + item.label + '</span></button>';
    }).join('');
    const mobile = NAV.filter(function (item) { return item.id !== 'requests'; });
    bottom.innerHTML = '<span class="nav-slider" aria-hidden="true"></span>' + mobile.map(function (item) {
      return '<button class="bottom-item" data-view="' + item.id + '" type="button">' +
        '<span class="nav-glyph" data-icon="' + item.glyph + '"></span>' +
        '<span class="nav-label">' + item.label + '</span></button>';
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
    const root = ['dns', 'logs', 'rules', 'settings'].indexOf(name) >= 0 ? 'tools' : name;
    ui.$$('.rail-item,.bottom-item').forEach(function (button) {
      button.classList.toggle('is-active', button.getAttribute('data-view') === root);
    });
    positionSlider(root);
  }

  /* Move the pill highlight under the active tab with a sliding transition. */
  function positionSlider(name) {
    const bottom = document.getElementById('bottom-nav');
    const slider = bottom && ui.$('.nav-slider', bottom);
    const active = bottom && ui.$('.bottom-item.is-active', bottom);
    if (!slider || !active || !isMobile()) return;
    const offset = active.offsetLeft + (active.offsetWidth - slider.offsetWidth) / 2;
    slider.style.transform = 'translateX(' + offset + 'px)';
  }

  function applyTheme(theme) {
    const next = theme || cfg.theme;
    cfg.theme = next;
    document.documentElement.setAttribute('data-theme', next);
    const meta = document.querySelector('meta[name="theme-color"]');
    if (meta) meta.setAttribute('content', next === 'dark' ? '#0b0b0d' : '#eef0f3');
    try { window.localStorage.setItem('flclash.theme', next); } catch (error) { /* ignore */ }
    const toggle = document.getElementById('rail-theme');
    if (toggle) toggle.innerHTML = icon('theme');
  }

  function setTheme(theme) {
    applyTheme(theme);
    if (current === 'settings') renderView('settings', true);
  }

  function modeLabel(mode) {
    return mode === 'global' ? '全局' : mode === 'direct' ? '直连' : '规则';
  }

  function subtitleFor(status) {
    if (!status) return '正在连接后端…';
    if (!status.running) return '内核已停止 · 功能保持可用';
    return '运行中 · ' + ui.duration(status.uptime) + ' · ' + modeLabel(status.mode) + '模式';
  }

  function updateChrome(status) {
    state.status = status;
    state.offline = false;
    const sideStatus = document.getElementById('rail-status');
    if (sideStatus) {
      sideStatus.classList.toggle('is-off', !status.running);
      sideStatus.innerHTML = icon(status.running ? 'status' : 'stop');
      sideStatus.setAttribute('title', status.running ? '内核运行中' : '内核已停止');
    }
    const version = document.querySelector('.rail-version');
    if (version) version.textContent = 'v' + (status.version || '—');
    if (current === 'dashboard') setHeader(META.dashboard, subtitleFor(status));
    const button = document.getElementById('core-status');
    button.className = 'round-action ' + (status.running ? 'is-ok' : 'is-warn');
    button.innerHTML = icon(status.running ? 'check' : 'refresh');
    button.setAttribute('aria-label', status.running ? '内核运行中' : '内核已停止');
    button.setAttribute('title', status.running ? '内核运行中' : '内核已停止');
    const traffic = status.traffic || {};
    headerTraffic.innerHTML =
      '<span class="traffic-up">' + icon('arrowUp', 14) + '<b>' + ui.rate(traffic.upRate || 0) + '</b></span>' +
      '<span class="traffic-down">' + icon('arrowDown', 14) + '<b>' + ui.rate(traffic.downRate || 0) + '</b></span>';
    if (miniUp) miniUp.textContent = ui.rate(traffic.upRate || 0);
    if (miniDown) miniDown.textContent = ui.rate(traffic.downRate || 0);
  }

  function markOffline(error) {
    if (state.offline) return;
    state.offline = true;
    ui.toast('后端不可达: ' + (error && error.message ? error.message : '网络错误'), 'error');
  }

  async function refreshStatus() {
    try {
      const status = await api.status();
      updateChrome(status);
      const traffic = status.traffic || {};
      const history = traffic.history;
      if (history && history.up && history.down) {
        state.history.up = history.up.slice(-60);
        state.history.down = history.down.slice(-60);
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

  function statusCard(status) {
    const running = !!status.running;
    const quality = status.quality || {};
    const knownIp = status.outboundIp && status.outboundIp !== '—';
    const info = status.ipInfo || {};
    const locationText = [info.country || '', info.city || ''].filter(Boolean).join(' · ');
    return '<section class="card status-card span-2' + (running ? ' is-running' : '') + '" data-role="status">' +
      '<div class="status-inner">' +
        '<div class="status-main">' +
          '<div class="status-head">' +
            '<span class="glyph" data-icon="status"></span>' +
            '<span class="card-title">状态</span>' +
            '<span class="chip ' + (running ? 'is-ok' : '') + '" data-role="state-chip">' +
              (running ? '<i class="chip-dot"></i>运行中' : '<i class="chip-dot"></i>已停止') + '</span>' +
          '</div>' +
          '<div class="status-ip-row">' +
            '<div class="country-badge">' +
              flower(52, 'flower-mask') +
              '<span class="country-code" data-role="country">' + ui.esc((status.countryCode || '--').toUpperCase()) + '</span>' +
            '</div>' +
            '<div class="ip-stack">' +
              '<button class="ip-text" data-role="ip" type="button" title="复制出口 IP">' +
                (knownIp ? ui.esc(status.outboundIp) : '<span class="sk-line" style="width:220px"></span>') + '</button>' +
              '<div class="ip-chips">' +
                '<span class="chip">' + ui.esc(quality.type || '直连') + '</span>' +
                '<span class="chip ' + (quality.level === '干净' || quality.level === '质量优' ? 'is-ok' : 'is-warn') + '" data-role="quality">' +
                  ui.esc(quality.level || '检测中') + '</span>' +
                (locationText ? '<span class="chip chip-plain" data-role="location">' + ui.esc(locationText) + '</span>' : '<span class="chip chip-plain" data-role="location" style="display:none"></span>') +
              '</div>' +
            '</div>' +
          '</div>' +
          '<div class="status-cells">' +
            '<div class="status-cell"><div class="status-cell-label">运行时长</div>' +
              '<div class="status-cell-value" data-role="uptime">' + (running ? ui.duration(status.uptime) : '—') + '</div></div>' +
            '<div class="status-cell"><div class="status-cell-label">IP 资料</div>' +
              '<div class="status-cell-value is-muted" data-role="org" title="' + ui.esc([info.isp || info.org || '', info.asn || ''].filter(Boolean).join(' · ')) + '">' +
                ui.esc(info.isp || info.org || status.organization || '—') + '</div></div>' +
          '</div>' +
        '</div>' +
        '<button class="start-toggle' + (running ? ' is-running' : '') + '" data-act="toggle" type="button" ' +
          'aria-label="' + (running ? '停止内核' : '启动内核') + '" title="' + (running ? '停止内核' : '启动内核') + '">' +
          flower(88, 'flower-bg') + '<span class="start-glyph">' + icon(running ? 'pause' : 'play', 30) + '</span></button>' +
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
      '<div class="speed-head">' +
        '<span class="speed-label">' + icon(up ? 'arrowUp' : 'arrowDown', 16) + '<span>' + (up ? '上传' : '下载') + '</span></span>' +
        '<span class="peak" data-role="peak">峰值 ' + ui.rate(peak || 0) + '</span>' +
      '</div>' +
      '<div class="speed-figure"><span class="speed-value" data-role="value">' + value.value + '</span>' +
        '<span class="speed-unit" data-role="unit">' + value.unit + '/s</span></div>' +
      '<div class="speed-total" data-role="total">累计 ' + ui.bytes(total || 0) + '</div>' +
      '<div class="speed-chart ' + (up ? 'is-up' : 'is-down') + '" data-role="chart"></div>' +
    '</section>';
  }

  function mobileSpeedCard(status) {
    const traffic = status.traffic || {};
    function tile(up) {
      const value = ui.splitBytes(up ? traffic.upRate : traffic.downRate);
      return '<div class="speed-tile">' +
        '<span class="speed-label">' + icon(up ? 'arrowUp' : 'arrowDown', 16) + '<span>' + (up ? '上传' : '下载') + '</span></span>' +
        '<div class="speed-figure"><span class="speed-value" data-role="m-' + (up ? 'up' : 'down') + '">' + value.value + '</span>' +
          '<span class="speed-unit" data-role="m-unit-' + (up ? 'up' : 'down') + '">' + value.unit + '/s</span></div>' +
        '<div class="speed-total">累计 ' + ui.bytes(up ? traffic.totalUp : traffic.totalDown) + '</div>' +
        '<div class="speed-chart ' + (up ? 'is-up' : 'is-down') + '" data-role="m-chart-' + (up ? 'up' : 'down') + '"></div>' +
      '</div>';
    }
    return '<section class="card"><div class="card-head"><span class="glyph" data-icon="speed"></span>' +
      '<span class="card-title">网络速度</span></div>' +
      '<div class="speed-tiles">' + tile(true) + tile(false) + '</div></section>';
  }

  function modeBar(status) {
    const modes = [
      ['rule', '规则', 'split'],
      ['global', '全局', 'proxies'],
      ['direct', '直连', 'target']
    ];
    return '<section class="card mode-bar span-2" data-role="mode-bar">' +
      '<span class="mode-title">' + icon('split', 18) + '<span>出站模式</span></span>' +
      '<div class="segmented" data-role="mode">' + modes.map(function (item) {
        return '<button class="segment' + (status.mode === item[0] ? ' is-active' : '') + '" data-mode="' + item[0] + '" type="button">' +
          '<span class="seg-icon" data-icon="' + item[2] + '"></span><span>' + item[1] + '</span></button>';
      }).join('') + '</div>' +
    '</section>';
  }

  function takeoverTile(kind, status) {
    const system = kind === 'systemProxy';
    const on = system ? !!status.systemProxy : !!status.tun;
    return '<section class="card takeover-tile" data-role="takeover-' + kind + '">' +
      '<span class="tile-icon' + (on ? ' is-on' : '') + '" data-role="icon" data-icon="' + (system ? 'shuffle' : 'shield') + '"></span>' +
      '<span class="tile-copy"><span class="tile-title">' + (system ? '系统代理' : '虚拟网卡') + '</span>' +
        '<span class="tile-sub" data-role="sub">' + (system ? '端口 ' + (status.mixedPort || '—') : (on ? '已接管全局流量' : '未启用')) + '</span></span>' +
      ui.switchHtml(kind, on, system ? '系统代理' : '虚拟网卡') +
    '</section>';
  }

  function intranetCard(status) {
    const on = !!status.allowLan;
    return '<section class="card intranet-card" data-role="intranet">' +
      ui.head('内网 IP', {
        glyph: 'devices',
        end: '<button class="icon-button" data-act="copy-ip" type="button" aria-label="复制内网 IP">' + icon('copy', 16) + '</button>'
      }) +
      '<div class="ip-figure" data-role="lan-ip">' + ui.esc(status.localIp || '—') + '</div>' +
      '<div class="sub-switch">' +
        '<span class="tile-icon' + (on ? ' is-on' : '') + '" data-icon="wifi"></span>' +
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
        '<div class="usage-row"><strong>' + (share * 100).toFixed(1).replace(/\.0$/, '') + '% 已用</strong>' +
          '<span>' + ui.esc(profile.used || '0 B') + ' / ' + ui.esc(profile.total || '0 B') + '</span></div>' +
        '<svg class="wave" viewBox="0 0 300 12" preserveAspectRatio="none">' + ui.wave(share, 300, 12) + '</svg>' +
      '</div>' +
    '</section>';
  }

  function serviceCard(status) {
    const services = status.services || [];
    return '<section class="card is-tappable" data-role="service" data-go="proxies">' +
      ui.head('服务状态', { glyph: 'verified', chevron: true }) +
      '<div class="service-list" data-role="service-list">' + (services.length ? services.map(function (item) {
        return '<div class="service-row">' + ui.letterTile(item.label) +
          '<span class="tile-copy"><span class="tile-title">' + ui.esc(item.label) + '</span></span>' +
          '<span class="service-value' + (item.ok ? '' : ' is-bad') + '">' +
          (item.delay ? ui.esc(item.delay) + ' ms' : '超时') + '</span></div>';
      }).join('') : '<div class="service-row"><span class="tile-copy"><span class="tile-title">内核 ' +
        (status.running ? '运行中' : '已停止') + '</span></span></div>') + '</div>' +
    '</section>';
  }

  function memoryCard(status) {
    const memory = status.memory || { app: 0, core: 0 };
    const total = (memory.app || 0) + (memory.core || 0);
    const appShare = total ? (memory.app / total) * 100 : 50;
    return '<section class="card is-tappable" data-role="memory">' +
      ui.head('内存信息', { glyph: 'memory', chevron: true }) +
      '<div class="memory-value"><span data-role="memory-total">' + ui.bytes(total) + '</span><span class="memory-unit">RSS</span></div>' +
      '<div class="memory-bar"><span class="memory-seg app" data-role="mem-app" style="width:' + appShare + '%"></span>' +
        '<span class="memory-seg core" data-role="mem-core" style="width:' + (100 - appShare) + '%"></span></div>' +
      '<div class="memory-legend">' +
        '<span class="legend-item"><i class="legend-dot"></i>应用 <b data-role="legend-app">' + ui.bytes(memory.app || 0) + '</b></span>' +
        '<span class="legend-item"><i class="legend-dot core"></i>内核 <b data-role="legend-core">' + ui.bytes(memory.core || 0) + '</b></span>' +
      '</div>' +
    '</section>';
  }

  function connectionsCard(status) {
    const stats = status.stats || {};
    return '<section class="card is-flush is-tappable" data-role="connections" data-go="requests">' +
      '<div class="stat-pairs">' +
        '<span class="stat-pill is-primary"><span class="stat-count" data-role="conn-count">' + ui.count(stats.connections || 0) + '</span>' +
          '<span class="stat-label">实时连接</span></span>' +
      '</div>' +
      '<div class="table-wrap" data-role="connections-table">' + ui.empty('暂无连接', 'requests') + '</div>' +
    '</section>';
  }

  function groupsCard(status) {
    const info = status.proxyGroups || { groups: [] };
    return '<section class="card is-tappable" data-role="groups" data-go="proxies">' +
      ui.head('策略组', {
        glyph: 'layers',
        chevron: true,
        end: '<span class="chip">' + (info.count || 0) + '</span>' +
          (info.timeout ? '<span class="chip is-warn">' + info.timeout + ' 超时</span>' : '')
      }) +
      '<div class="group-list" data-role="group-list">' + (info.groups || []).slice(0, 6).map(function (group) {
        return '<div class="group-row">' + ui.letterTile(group.name) +
          '<span class="group-copy"><span class="group-name">' + ui.esc(group.name) + '</span>' +
            '<span class="group-now">' + ui.esc(group.now || '—') + '</span></span>' +
          '<span class="chip">' + ui.esc(group.count || 0) + '</span></div>';
      }).join('') + '</div>' +
    '</section>';
  }

  function dashLayout() {
    try { return JSON.parse(window.localStorage.getItem('flclash.dashcards')) || {}; }
    catch (error) { return {}; }
  }

  function applyDashLayout(node) {
    const layout = dashLayout();
    const map = {
      speed: '[data-role="speed-up"],[data-role="speed-down"],.dash-row.is-control, .card:has(>.speed-tiles)',
      mode: '[data-role="mode-bar"]',
      takeover: '[data-role="takeover-systemProxy"],[data-role="takeover-tun"]',
      intranet: '[data-role="intranet"]',
      profile: '[data-role="profile"]',
      service: '[data-role="service"]',
      memory: '[data-role="memory"]',
      tables: '[data-role="connections"],[data-role="groups"]'
    };
    Object.keys(map).forEach(function (key) {
      if (layout[key] === false) {
        ui.$$(map[key], node).forEach(function (item) { item.classList.add('is-hidden'); });
      }
    });
  }

  function bindLayoutEditor(node) {
    const button = document.getElementById('layout-edit');
    if (!button) return;
    button.addEventListener('click', function () {
      const layout = dashLayout();
      const items = [
        ['speed', '网络速度'], ['mode', '出站模式'], ['takeover', '系统代理 / 虚拟网卡'],
        ['intranet', '内网 IP'], ['profile', '配置'], ['service', '服务状态'],
        ['memory', '内存信息'], ['tables', '连接与策略组']
      ];
      ui.openSheet('编辑仪表盘', '<div class="settings-card">' + items.map(function (item) {
        const on = layout[item[0]] !== false;
        return '<div class="settings-row"><div class="row-copy"><div class="row-title">' + item[1] + '</div></div>' +
          ui.switchHtml('dash-' + item[0], on, item[1]) + '</div>';
      }).join('') + '</div><p class="sheet-note">关闭的卡片将从仪表盘中隐藏,设置保存在本设备。</p>', function (body) {
        ui.$$('[data-switch]', body).forEach(function (sw) {
          sw.addEventListener('click', function () {
            const key = sw.getAttribute('data-switch').replace('dash-', '');
            const on = sw.classList.contains('is-on');
            sw.classList.toggle('is-on', !on);
            const next = dashLayout();
            next[key] = !on;
            try { window.localStorage.setItem('flclash.dashcards', JSON.stringify(next)); } catch (error) { /* ignore */ }
            ui.$$('[data-role]', node).forEach(function () { /* noop */ });
            applyDashLayout(node);
          });
        });
      });
    });
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
        memoryCard(status),
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
          .then(function () {
            if (key === 'tun') {
              const sub = ui.$('[data-role="takeover-tun"] [data-role="sub"]', node);
              if (sub) sub.textContent = on ? '已接管全局流量' : '未启用';
            }
            return refreshStatus();
          })
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
    if (ipButton) ipButton.addEventListener('click', function () { copy(ipButton.textContent.trim()); });
    const copyButton = ui.$('[data-act="copy-ip"]', node);
    if (copyButton) copyButton.addEventListener('click', function () { copy((state.status && state.status.localIp) || ''); });

    ui.$$('[data-go]', node).forEach(function (item) {
      ui.pressable(item, function () { navigate(item.getAttribute('data-go')); });
    });
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
      chip.innerHTML = '<i class="chip-dot"></i>' + (running ? '运行中' : '已停止');
    }
    const country = ui.$('[data-role="country"]', node);
    if (country) country.textContent = (status.countryCode || '--').toUpperCase();
    const ip = ui.$('[data-role="ip"]', node);
    if (ip && status.outboundIp && ip.textContent !== status.outboundIp) {
      ip.textContent = status.outboundIp;
    }
    const uptime = ui.$('[data-role="uptime"]', node);
    if (uptime) uptime.textContent = running ? ui.duration(status.uptime) : '—';
    const org = ui.$('[data-role="org"]', node);
    const info = status.ipInfo || {};
    if (org) {
      org.textContent = info.isp || info.org || status.organization || '—';
      org.setAttribute('title', [info.isp || info.org || '', info.asn || ''].filter(Boolean).join(' · '));
    }
    const qualityChip = ui.$('[data-role="quality"]', node);
    const qualityNow = status.quality || {};
    if (qualityChip) {
      qualityChip.textContent = qualityNow.level || '—';
      qualityChip.className = 'chip ' + (qualityNow.level === '干净' || qualityNow.level === '质量优' ? 'is-ok' : 'is-warn');
    }
    const locationChip = ui.$('[data-role="location"]', node);
    if (locationChip) {
      const loc = [info.country || '', info.city || ''].filter(Boolean).join(' · ');
      locationChip.textContent = loc;
      locationChip.style.display = loc ? '' : 'none';
    }

    const toggle = ui.$('[data-act="toggle"]', node);
    if (toggle) {
      toggle.classList.toggle('is-running', running);
      toggle.innerHTML = flower(88, 'flower-bg') + '<span class="start-glyph">' + icon(running ? 'pause' : 'play', 30) + '</span>';
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
      if (mobileValue) ui.countUp(mobileValue, Number(value.value), function (v) { return v.toFixed(1); });
      const mobileUnit = ui.$('[data-role="m-unit-' + dir + '"]', node);
      if (mobileUnit) mobileUnit.textContent = value.unit + '/s';
      const mobileChart = ui.$('[data-role="m-chart-' + dir + '"]', node);
      if (mobileChart) ui.renderChart(mobileChart, dir === 'up' ? state.history.up : state.history.down, { height: 46, minScale: 8192 });
    });

    ui.$$('[data-mode]', node).forEach(function (button) {
      button.classList.toggle('is-active', button.getAttribute('data-mode') === status.mode);
    });

    ['systemProxy', 'tun'].forEach(function (kind) {
      const tile = ui.$('[data-role="takeover-' + kind + '"]', node);
      if (!tile) return;
      const on = !!status[kind];
      const sw = ui.$('[data-switch]', tile);
      if (sw) { sw.classList.toggle('is-on', on); sw.setAttribute('aria-checked', String(on)); }
      const iconNode = ui.$('[data-role="icon"]', tile);
      if (iconNode) iconNode.classList.toggle('is-on', on);
      const sub = ui.$('[data-role="sub"]', tile);
      if (sub && kind === 'tun') sub.textContent = on ? '已接管全局流量' : '未启用';
      if (sub && kind === 'systemProxy') sub.textContent = '端口 ' + (status.mixedPort || '—');
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

    const connCount = ui.$('[data-role="conn-count"]', node);
    if (connCount) connCount.textContent = ui.count(status.stats ? status.stats.connections : 0);

    const table = ui.$('[data-role="connections-table"]', node);
    if (table) {
      api.connections().then(function (list) {
        if (!list || !list.length) return;
        table.innerHTML = connectionsTable(list.slice(0, 8));
      }).catch(function () {});
    }

    /* Service / memory / profile cards must track live status too, not just
     * the values captured at first render. */
    const serviceList = ui.$('[data-role="service-list"]', node);
    if (serviceList) {
      const services = status.services || [];
      serviceList.innerHTML = services.length ? services.map(function (item) {
        return '<div class="service-row">' + ui.letterTile(item.label) +
          '<span class="tile-copy"><span class="tile-title">' + ui.esc(item.label) + '</span></span>' +
          '<span class="service-value' + (item.ok ? '' : ' is-bad') + '">' +
          (item.delay ? ui.esc(item.delay) + ' ms' : '超时') + '</span></div>';
      }).join('') : '<div class="service-row">' + ui.letterTile(running ? 'M' : 'X') +
        '<span class="tile-copy"><span class="tile-title">' + (running ? '内核运行中' : '内核已停止') + '</span></span>' +
        '<span class="service-value' + (running ? '' : ' is-bad') + '">' + (running ? '正常' : '离线') + '</span></div>';
    }

    const memoryTotal = ui.$('[data-role="memory-total"]', node);
    const memory = status.memory || { app: 0, core: 0 };
    if (memoryTotal) memoryTotal.textContent = ui.bytes((memory.app || 0) + (memory.core || 0));
    const memApp = ui.$('[data-role="mem-app"]', node);
    const memCore = ui.$('[data-role="mem-core"]', node);
    const memSum = (memory.app || 0) + (memory.core || 0);
    if (memApp && memCore && memSum) {
      const appShare = (memory.app / memSum) * 100;
      memApp.style.width = appShare + '%';
      memCore.style.width = (100 - appShare) + '%';
    }
    const legendApp = ui.$('[data-role="legend-app"]', node);
    if (legendApp) legendApp.textContent = ui.bytes(memory.app || 0);
    const legendCore = ui.$('[data-role="legend-core"]', node);
    if (legendCore) legendCore.textContent = ui.bytes(memory.core || 0);

    const profileChip = ui.$('[data-role="profile"] .card-end .chip', node);
    if (profileChip && status.profile) profileChip.textContent = (status.profile.proxies || 0) + ' 节点';
    const profileMeta = ui.$('[data-role="profile"] .profile-meta', node);
    if (profileMeta && status.profile) {
      profileMeta.innerHTML = '<span>更新于 ' + ui.esc(status.profile.updatedAt || '—') + '</span>' +
        (status.profile.expireDays ? '<span>剩余 ' + ui.esc(status.profile.expireDays) + ' 天</span>' : '');
    }
    const usageRow = ui.$('[data-role="profile"] .usage-row', node);
    if (usageRow && status.profile && status.profile.total !== undefined) {
      const share = Number(status.profile.usedShare) || 0;
      usageRow.innerHTML = '<strong>' + (share * 100).toFixed(1).replace(/\.0$/, '') + '% 已用</strong>' +
        '<span>' + ui.esc(status.profile.used || '0 B') + ' / ' + ui.esc(status.profile.total || '0 B') + '</span>';
    }

    const groupsCardNode = ui.$('[data-role="groups"]', node);
    if (groupsCardNode && status.proxyGroups) {
      const info = status.proxyGroups;
      const countChipNode = ui.$('.card-end .chip', groupsCardNode);
      if (countChipNode) countChipNode.textContent = String(info.count || 0);
      const warnChip = ui.$('.card-end .chip.is-warn', groupsCardNode);
      if (info.timeout && !warnChip) {
        ui.$('.card-end', groupsCardNode).insertAdjacentHTML('afterbegin',
          '<span class="chip is-warn">' + info.timeout + ' 超时</span>');
      }
      const gl = ui.$('[data-role="group-list"]', groupsCardNode);
      if (gl) {
        gl.innerHTML = (info.groups || []).slice(0, 6).map(function (group) {
          return '<div class="group-row">' + ui.letterTile(group.name) +
            '<span class="group-copy"><span class="group-name">' + ui.esc(group.name) + '</span>' +
              '<span class="group-now">' + ui.esc(group.now || '—') + '</span></span>' +
            '<span class="chip">' + ui.esc(group.count || 0) + '</span></div>';
        }).join('');
      }
    }
  }

  function connectionsTable(list) {
    return '<table class="data"><thead><tr>' +
      '<th>目标</th><th>协议</th><th>出口</th><th class="num">下载</th><th class="num">上传</th><th>时长</th>' +
      '</tr></thead><tbody>' + list.map(function (item) {
        return '<tr>' +
          '<td title="' + ui.esc(item.host) + '">' + ui.esc(item.host) + ':' + ui.esc(item.port) + '</td>' +
          '<td class="is-muted">' + ui.esc((item.network || 'tcp').toUpperCase()) + '</td>' +
          '<td>' + ui.esc(item.chain || '-') + '</td>' +
          '<td class="num">' + ui.bytes(item.down) + '</td>' +
          '<td class="num">' + ui.bytes(item.up) + '</td>' +
          '<td class="is-muted">' + ui.duration(item.duration) + '</td>' +
          '</tr>';
      }).join('') + '</tbody></table>';
  }

  async function renderDashboard() {
    const token = ++mountToken;
    dashboardNode = renderDashboardShell(state.status || { running: false, proxyGroups: {} });
    viewRoot.innerHTML = '';
    viewRoot.appendChild(dashboardNode);
    applyDashLayout(dashboardNode);
    window.FlClashIcons.mount(dashboardNode);
    bindDashboard(dashboardNode);
    bindLayoutEditor(dashboardNode);
    ui.every('status', function () { refreshStatus().catch(function () {}); }, cfg.pollInterval);
    ui.every('dash-connections', function () {
      api.connections().then(function (list) {
        if (token !== mountToken || !dashboardNode) return;
        state.connections = list;
        const table = ui.$('[data-role="connections-table"]', dashboardNode);
        if (table && list && list.length) table.innerHTML = connectionsTable(list.slice(0, 8));
      }).catch(function () {});
    }, 3000);
  }

  /* ----------------------------------------------------------------- router */

  function setHeader(title, subtitle) {
    pageTitle.textContent = title;
    pageSub.textContent = subtitle || '';
    document.querySelector('.app-header').classList.toggle('has-sub', !!subtitle);
  }

  function skeleton() {
    viewRoot.innerHTML = '<div class="view page">' + ui.skeletonRows(6) + '</div>';
  }

  async function renderView(name, force) {
    if (name === 'dashboard') { await renderDashboard(); return; }
    ui.stopAll();
    if (!force) skeleton();
    const builder = views[name];
    const token = ++mountToken;
    const ctx = {
      api: api,
      state: state,
      navigate: function (next) { window.location.hash = '#/' + next; },
      refreshStatus: refreshStatus,
      reloadProfiles: async function () {
        state.profiles = await api.profiles().catch(function () { return state.profiles; });
        if (token === mountToken && ctx.onProfiles) ctx.onProfiles();
      },
      reloadProxies: async function () {
        state.proxies = await api.proxies().catch(function () { return state.proxies; });
        if (token === mountToken && ctx.onProxies) ctx.onProxies();
      },
      reloadConnections: async function () {
        state.connections = await api.connections().catch(function () { return state.connections; });
        if (token === mountToken && ctx.onConnections) ctx.onConnections();
      },
      setTheme: setTheme,
      copy: copy
    };
    const node = builder(ctx);
    viewRoot.innerHTML = '';
    viewRoot.appendChild(node);
    window.FlClashIcons.mount(node);
  }

  function navigate(name) {
    if (!META[name]) name = 'dashboard';
    if (current === name && name !== 'dashboard') return;
    const splash = document.getElementById('boot-splash');
    if (splash) { splash.classList.add('is-done'); window.setTimeout(function () { splash.remove(); }, 500); }
    current = name;
    paintNav(name);
    ui.stopAll();
    setHeader(META[name], '');
    viewRoot.classList.remove('view-in');
    renderView(name).then(function () {
      window.requestAnimationFrame(function () { viewRoot.classList.add('view-in'); });
    }).catch(function (error) {
      viewRoot.innerHTML = '<div class="view page">' + ui.empty(error.message, 'info') + '</div>';
    });
  }

  window.addEventListener('hashchange', function () {
    navigate((window.location.hash || '#/dashboard').replace('#/', ''));
  });
  window.addEventListener('resize', ui.debounce(function () {
    if (current === 'dashboard') { renderDashboard(); }
    positionSlider(current);
  }, 250));

  buildNav();
  applyTheme();
  window.FlClashIcons.mount(document);
  navigate((window.location.hash || '#/dashboard').replace('#/', ''));
  refreshStatus().catch(function () {});
})();
