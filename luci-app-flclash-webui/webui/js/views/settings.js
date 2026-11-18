/* View: 设置 — networking / core / appearance / service, persisted to
 * config.yaml by the backend so settings survive restarts (and work while
 * the core is stopped). */
(function () {
  'use strict';

  const ui = window.FlClashUI;
  const icon = window.FlClashIcons.svg;

  function settings(ctx) {
    const root = document.createElement('div');
    root.className = 'view page';
    const cfg = ctx.state.config || {};
    const status = ctx.state.status || {};

    root.innerHTML =
      '<div class="settings-group"><div class="settings-title">网络</div>' +
        '<div class="settings-card">' +
          '<div class="settings-row"><div class="row-copy"><div class="row-title">出站模式</div><div class="row-desc">规则 / 全局 / 直连</div></div>' +
            '<select class="select" data-cfg="mode"><option value="rule">规则</option><option value="global">全局</option><option value="direct">直连</option></select></div>' +
          '<div class="settings-row"><div class="row-copy"><div class="row-title">混合端口</div><div class="row-desc">HTTP / SOCKS 共用端口</div></div>' +
            '<input class="input input-num" type="number" min="1" max="65535" data-cfg="mixedPort"></div>' +
          '<div class="settings-row"><div class="row-copy"><div class="row-title">允许局域网连接</div><div class="row-desc">其它设备可经本机代理</div></div>' +
            '<button class="switch" role="switch" data-cfg="allowLan" type="button"><span class="thumb"></span></button></div>' +
          '<div class="settings-row"><div class="row-copy"><div class="row-title">IPv6</div><div class="row-desc">解析并代理 IPv6 流量</div></div>' +
            '<button class="switch" role="switch" data-cfg="ipv6" type="button"><span class="thumb"></span></button></div>' +
        '</div></div>' +
      '<div class="settings-group"><div class="settings-title">透明代理(局域网接管)</div>' +
        '<div class="settings-card">' +
          '<div class="settings-row"><div class="row-copy"><div class="row-title">接管局域网流量</div>' +
            '<div class="row-desc" data-role="proxy-desc">TCP/UDP/DNS 经内核分流;路由器管理面不受影响</div></div>' +
            '<button class="switch" role="switch" data-cfg="proxyLan" type="button"><span class="thumb"></span></button></div>' +
          '<div class="settings-row"><div class="row-copy"><div class="row-title">当前状态</div><div class="row-desc" data-role="proxy-state">—</div></div>' +
            '<span class="chip" data-role="proxy-chip">—</span></div>' +
        '</div></div>' +
      '<div class="settings-group"><div class="settings-title">DNS(进阶)</div>' +
        '<div class="settings-card">' +
          '<div class="settings-row"><div class="row-copy"><div class="row-title">启用 DNS 模块</div><div class="row-desc">关闭后由系统 DNS 解析</div></div>' +
            '<button class="switch" role="switch" data-cfg="dnsEnable" type="button"><span class="thumb"></span></button></div>' +
          '<div class="settings-row"><div class="row-copy"><div class="row-title">解析增强模式</div><div class="row-desc">fake-ip 或 redir-host</div></div>' +
            '<select class="select" data-cfg="dnsMode"><option value="fake-ip">fake-ip</option><option value="redir-host">redir-host</option></select></div>' +
          '<div class="settings-row"><div class="row-copy"><div class="row-title">域名解析服务器</div><div class="row-desc">每行一个,支持 udp / DoH(https://) / DoT(tls://)</div></div></div>' +
          '<textarea class="input dns-area" data-cfg="dnsNameservers" rows="4" spellcheck="false"></textarea>' +
          '<div class="settings-row"><div class="row-copy"><div class="row-title">默认 DNS(bootstrap)</div><div class="row-desc">用于解析 DoH/DoT 域名,每行一个</div></div></div>' +
          '<textarea class="input dns-area" data-cfg="dnsDefaults" rows="2" spellcheck="false"></textarea>' +
          '<div class="settings-row"><div class="row-copy"><div class="row-title">fake-ip 范围</div><div class="row-desc">默认 198.18.0.1/16</div></div>' +
            '<input class="input input-num" type="text" data-cfg="dnsFakeRange"></div>' +
          '<div class="settings-row"><div class="row-copy"><div class="row-title">fake-ip 过滤</div><div class="row-desc">不走 fake-ip 的域名,每行一个</div></div></div>' +
          '<textarea class="input dns-area" data-cfg="dnsFilter" rows="3" spellcheck="false"></textarea>' +
        '</div></div>' +
      '<div class="settings-group"><div class="settings-title">内核</div>' +
        '<div class="settings-card">' +
          '<div class="settings-row"><div class="row-copy"><div class="row-title">虚拟网卡(TUN)</div><div class="row-desc">接管全局流量,需内核支持</div></div>' +
            '<button class="switch" role="switch" data-cfg="tun" type="button"><span class="thumb"></span></button></div>' +
          '<div class="settings-row"><div class="row-copy"><div class="row-title">日志级别</div><div class="row-desc">影响日志详细程度</div></div>' +
            '<select class="select" data-cfg="logLevel"><option value="debug">debug</option><option value="info">info</option><option value="warning">warning</option><option value="error">error</option></select></div>' +
          '<div class="settings-row"><div class="row-copy"><div class="row-title">核心版本</div><div class="row-desc" data-role="core-version">—</div></div>' +
            '<span class="chip chip-plain">mihomo</span></div>' +
        '</div></div>' +
      '<div class="settings-group"><div class="settings-title">外观</div>' +
        '<div class="settings-card">' +
          '<div class="settings-row"><div class="row-copy"><div class="row-title">深色模式</div><div class="row-desc">切换 WebUI 主题</div></div>' +
            '<button class="switch" role="switch" data-cfg="theme" type="button"><span class="thumb"></span></button></div>' +
        '</div></div>' +
      '<div class="settings-group"><div class="settings-title">服务</div>' +
        '<div class="settings-card">' +
          '<div class="settings-row"><div class="row-copy"><div class="row-title">运行状态</div><div class="row-desc" data-role="state">—</div></div>' +
            '<button class="btn" data-act="toggle" type="button">—</button></div>' +
          '<div class="settings-row"><div class="row-copy"><div class="row-title">重启内核</div><div class="row-desc">保留配置,重建连接</div></div>' +
            '<button class="btn" data-act="restart" type="button">重启</button></div>' +
        '</div></div>' +
      '<div class="page-toolbar" style="justify-content:flex-end"><button class="btn is-primary" data-act="save" type="button">' + icon('check', 16) + '保存设置</button></div>';

    const controls = {
      mode: ui.$('[data-cfg="mode"]', root),
      mixedPort: ui.$('[data-cfg="mixedPort"]', root),
      logLevel: ui.$('[data-cfg="logLevel"]', root),
      allowLan: ui.$('[data-cfg="allowLan"]', root),
      ipv6: ui.$('[data-cfg="ipv6"]', root),
      tun: ui.$('[data-cfg="tun"]', root),
      dnsEnable: ui.$('[data-cfg="dnsEnable"]', root),
      dnsMode: ui.$('[data-cfg="dnsMode"]', root),
      dnsNameservers: ui.$('[data-cfg="dnsNameservers"]', root),
      dnsDefaults: ui.$('[data-cfg="dnsDefaults"]', root),
      dnsFakeRange: ui.$('[data-cfg="dnsFakeRange"]', root),
      dnsFilter: ui.$('[data-cfg="dnsFilter"]', root),
      theme: ui.$('[data-cfg="theme"]', root),
      proxyLan: ui.$('[data-cfg="proxyLan"]', root)
    };

    function fill() {
      const cfgNow = ctx.state.config || {};
      const statusNow = ctx.state.status || {};
      const dns = statusNow.dns || {};
      controls.mode.value = cfgNow.mode || statusNow.mode || 'rule';
      controls.mixedPort.value = cfgNow.mixedPort || statusNow.mixedPort || 7892;
      controls.logLevel.value = cfgNow.logLevel || statusNow.logLevel || 'info';
      ['allowLan', 'ipv6', 'tun'].forEach(function (key) {
        const value = cfgNow[key] != null ? cfgNow[key] : statusNow[key];
        controls[key].classList.toggle('is-on', !!value);
        controls[key].setAttribute('aria-checked', String(!!value));
      });
      controls.dnsEnable.classList.toggle('is-on', dns.enable !== false);
      controls.dnsEnable.setAttribute('aria-checked', String(dns.enable !== false));
      controls.dnsMode.value = dns['enhanced-mode'] || 'fake-ip';
      controls.dnsNameservers.value = (dns.nameserver || []).join('\n');
      controls.dnsDefaults.value = (dns['default-nameserver'] || []).join('\n');
      controls.dnsFakeRange.value = dns['fake-ip-range'] || '198.18.0.1/16';
      controls.dnsFilter.value = (dns['fake-ip-filter'] || []).join('\n');
      const core = ui.$('[data-role="core-version"]', root);
      if (core) core.textContent = statusNow.coreVersion || '—';
      const running = !!statusNow.running;
      const stateNode = ui.$('[data-role="state"]', root);
      if (stateNode) stateNode.textContent = running ? '运行中' : '已停止(设置仍可保存)';
      const toggle = ui.$('[data-act="toggle"]', root);
      if (toggle) {
        toggle.textContent = running ? '停止' : '启动';
        toggle.classList.toggle('is-danger', running);
      }
    }

    api.config().then(function (data) { ctx.state.config = data; fill(); }).catch(function () { fill(); });
    fill();

    ['allowLan', 'ipv6', 'tun', 'dnsEnable'].forEach(function (key) {
      controls[key].addEventListener('click', function () { controls[key].classList.toggle('is-on'); });
    });

    controls.proxyLan.addEventListener('click', function () {
      const sw = controls.proxyLan;
      if (sw.dataset.busy === '1') return;
      sw.dataset.busy = '1';
      const on = sw.classList.contains('is-on');
      const action = on ? 'stop' : 'start';
      sw.classList.toggle('is-on', !on);
      ctx.api.proxyToggle(action)
        .then(function () {
          ui.toast(action === 'start' ? '局域网接管已开启' : '局域网接管已关闭', 'ok');
        })
        .catch(function (error) {
          sw.classList.toggle('is-on', on);
          ui.toast((action === 'start' ? '开启失败: ' : '关闭失败: ') + error.message, 'error');
        })
        .then(function () { sw.dataset.busy = ''; });
    });
    controls.theme.classList.toggle('is-on', document.documentElement.getAttribute('data-theme') === 'dark');
    controls.theme.addEventListener('click', function () {
      const dark = !controls.theme.classList.contains('is-on');
      controls.theme.classList.toggle('is-on', dark);
      ctx.setTheme(dark ? 'dark' : 'light');
    });

    ui.$('[data-act="toggle"]', root).addEventListener('click', function () {
      const running = !!(ctx.state.status && ctx.state.status.running);
      ctx.api.service(running ? 'stop' : 'start')
        .then(function () { ui.toast(running ? '内核已停止' : '内核已启动', 'ok'); return ctx.refreshStatus(); })
        .then(fill)
        .catch(function (error) { ui.toast('操作失败: ' + error.message, 'error'); });
    });

    ui.$('[data-act="restart"]', root).addEventListener('click', function () {
      ctx.api.service('restart')
        .then(function () { ui.toast('内核已重启', 'ok'); return ctx.refreshStatus(); })
        .then(fill)
        .catch(function (error) { ui.toast('重启失败: ' + error.message, 'error'); });
    });

    ui.$('[data-act="save"]', root).addEventListener('click', function (event) {
      const button = event.currentTarget;
      button.disabled = true;
      const patch = {
        mode: controls.mode.value,
        mixedPort: Number(controls.mixedPort.value) || 7892,
        logLevel: controls.logLevel.value,
        allowLan: controls.allowLan.classList.contains('is-on'),
        ipv6: controls.ipv6.classList.contains('is-on'),
        tun: controls.tun.classList.contains('is-on'),
        dns: {
          enable: controls.dnsEnable.classList.contains('is-on'),
          'enhanced-mode': controls.dnsMode.value,
          nameserver: (controls.dnsNameservers.value || '').split('\n')
            .map(function (s) { return s.trim(); }).filter(Boolean),
          'default-nameserver': (controls.dnsDefaults.value || '').split('\n')
            .map(function (s) { return s.trim(); }).filter(Boolean),
          'fake-ip-range': controls.dnsFakeRange.value.trim() || '198.18.0.1/16',
          'fake-ip-filter': (controls.dnsFilter.value || '').split('\n')
            .map(function (s) { return s.trim(); }).filter(Boolean)
        }
      };
      ctx.api.saveConfig(patch)
        .then(function () {
          ctx.state.config = patch;
          ui.toast('设置已保存(重启后依然生效)', 'ok');
          return ctx.refreshStatus();
        })
        .catch(function (error) { ui.toast('保存失败: ' + error.message, 'error'); })
        .then(function () { button.disabled = false; });
    });

    return root;
  }

  window.FlClashViews = window.FlClashViews || {};
  window.FlClashViews.settings = settings;
})();
