/* View: 设置 — networking / core / appearance / service, persisted to
 * config.yaml by the backend so settings survive restarts (and work while
 * the core is stopped). */
(function () {
  'use strict';

  const ui = window.FlClashUI;
  const icon = window.FlClashIcons.svg;

  function settings(ctx) {
    const api = ctx.api;
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
          '<div class="settings-row"><div class="row-copy"><div class="row-title">解析增强模式</div><div class="row-desc">fake-ip 快速 / redir-host 真实 / 混合(嗅探)</div></div>' +
            '<select class="select" data-cfg="dnsMode"><option value="fake-ip">fake-ip</option><option value="redir-host">redir-host</option><option value="mixed">混合模式</option></select></div>' +
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
          '<div class="settings-row"><div class="row-copy"><div class="row-title">内核类型</div><div class="row-desc">mihomo(Meta) / sing-box 智能内核</div></div>' +
            '<select class="select" data-cfg="coreType"><option value="mihomo">mihomo (Meta)</option><option value="sing-box">sing-box</option></select></div>' +
          '<div class="settings-row"><div class="row-copy"><div class="row-title">已安装版本</div><div class="row-desc" data-role="core-version">—</div></div>' +
            '<button class="btn btn-sm" data-act="core-update" type="button">在线更新</button></div>' +
          '<div class="update-progress" data-role="update-progress" hidden>' +
            '<div class="update-bar"><span data-role="update-bar-fill"></span></div>' +
            '<pre class="update-log" data-role="update-log"></pre></div>' +
          '<div class="settings-row"><div class="row-copy"><div class="row-title">虚拟网卡(TUN)</div><div class="row-desc">接管全局流量,需内核支持</div></div>' +
            '<button class="switch" role="switch" data-cfg="tun" type="button"><span class="thumb"></span></button></div>' +
          '<div class="settings-row"><div class="row-copy"><div class="row-title">TUN 协议栈</div><div class="row-desc">gvisor 兼容 / system 性能 / mixed 均衡</div></div>' +
            '<select class="select" data-cfg="tunStack"><option value="mixed">mixed</option><option value="gvisor">gvisor</option><option value="system">system</option></select></div>' +
          '<div class="settings-row"><div class="row-copy"><div class="row-title">防火墙后端</div><div class="row-desc" data-role="firewall-backend">自动检测</div></div>' +
            '<span class="chip chip-plain" data-role="firewall-chip">—</span></div>' +
          '<div class="settings-row"><div class="row-copy"><div class="row-title">日志级别</div><div class="row-desc">影响日志详细程度</div></div>' +
            '<select class="select" data-cfg="logLevel"><option value="debug">debug</option><option value="info">info</option><option value="warning">warning</option><option value="error">error</option></select></div>' +
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
      proxyLan: ui.$('[data-cfg="proxyLan"]', root),
      coreType: ui.$('[data-cfg="coreType"]', root),
      tunStack: ui.$('[data-cfg="tunStack"]', root)
    };

    /* Installed cores come from the same-origin REST bridge (binary probe +
     * UCI), so the row never shows a hard-coded version. */
    function fillCoreInfo() {
      const node = ui.$('[data-role="core-version"]', root);
      if (!node) return;
      ctx.api.coreInfo().then(function (info) {
        const parts = [];
        parts.push(info.mihomo_installed ? (info.mihomo_version || 'mihomo 已安装') : 'mihomo 未安装');
        parts.push(info.singbox_installed ? (info.singbox_version || 'sing-box 已安装') : 'sing-box 未安装');
        node.textContent = parts.join(' · ') + ' · 运行中: ' + (info.active || 'mihomo');
        const select = controls.coreType;
        if (select) {
          ui.$$('option', select).forEach(function (opt) {
            const installed = opt.value === 'sing-box' ? info.singbox_installed : info.mihomo_installed;
            opt.textContent = (opt.value === 'sing-box' ? 'sing-box' : 'mihomo (Meta)') + (installed ? '' : '（未安装）');
          });
        }
      }).catch(function () { node.textContent = '读取失败'; });
    }

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
      controls.dnsMode.value = cfgNow.dnsMode || dns['enhanced-mode'] || 'fake-ip';
      controls.dnsNameservers.value = (dns.nameserver || []).join('\n');
      controls.dnsDefaults.value = (dns['default-nameserver'] || []).join('\n');
      controls.dnsFakeRange.value = dns['fake-ip-range'] || '198.18.0.1/16';
      controls.dnsFilter.value = (dns['fake-ip-filter'] || []).join('\n');
      controls.coreType.value = cfgNow.coreType || statusNow.coreType || 'mihomo';
      controls.tunStack.value = cfgNow.tunStack || statusNow.tunStack || 'mixed';
      const core = ui.$('[data-role="core-version"]', root);
      if (core) core.textContent = statusNow.coreVersion || '—';
      const fw = statusNow.firewallBackend || cfgNow.firewallBackend;
      const fwChip = ui.$('[data-role="firewall-chip"]', root);
      if (fwChip) fwChip.textContent = fw ? fw.toUpperCase() : '—';
      const fwDesc = ui.$('[data-role="firewall-backend"]', root);
      if (fwDesc) fwDesc.textContent = fw === 'iptables'
        ? 'iptables (TPROXY/REDIRECT)' : 'nftables (fw4) · 兼容 iptables';
      const running = !!statusNow.running;
      const stateNode = ui.$('[data-role="state"]', root);
      if (stateNode) stateNode.textContent = running ? '运行中' : '已停止(设置仍可保存)';
      const toggle = ui.$('[data-act="toggle"]', root);
      if (toggle) {
        toggle.textContent = running ? '停止' : '启动';
        toggle.classList.toggle('is-danger', running);
      }
    }

    /* The transparent-proxy switch is driven by the real proxy-status
     * endpoint, so a page reload reflects the actual nft state (the switch
     * used to always start off even when takeover was active). */
    function fillProxy() {
      if (!ctx.api.proxyStatus) return;
      ctx.api.proxyStatus().then(function (data) {
        const on = !!(data && (data.enabled || data.rulesActive));
        controls.proxyLan.classList.toggle('is-on', on);
        controls.proxyLan.setAttribute('aria-checked', String(on));
        const stateNode = ui.$('[data-role="proxy-state"]', root);
        if (stateNode) stateNode.textContent = on ? '已接管局域网 TCP/UDP/DNS' : '未接管,管理面不受影响';
        const chip = ui.$('[data-role="proxy-chip"]', root);
        if (chip) {
          chip.textContent = on ? '已开启' : '已关闭';
          chip.className = 'chip ' + (on ? 'is-ok' : '');
        }
        const desc = ui.$('[data-role="proxy-desc"]', root);
        if (desc && data) {
          desc.textContent = '接口 ' + (data.lanInterface || 'br-lan') +
            ' · REDIR ' + (data.redirPort || 7893) +
            ' · TPROXY ' + (data.tproxyPort || 7894) +
            ' · DNS ' + (data.dnsPort || 1053);
          // Inbound ports come from the read-only system-proxy contract so
          // the row shows what clients actually dial, not a guess.
          ctx.api.systemProxy().then(function (sp) {
            if (sp) desc.textContent += ' · 混合 ' + (sp.mixed_port || 7892) + ' · 控制 ' + (sp.clash_api_port || 9097);
          }).catch(function () { /* keep the firewall ports */ });
        }
      }).catch(function () { /* backend not available: leave switch untouched */ });
    }

    /* Lifecycle calls answer 202 {taskId}; wait for the task result so the
     * toast reflects the real outcome, not just that the request was queued. */
    function settleTask(reply) {
      if (reply && reply.taskId && ctx.comm) {
        return ctx.comm.waitTask(reply.taskId).then(function (task) {
          if (task && task.state === 'failed') throw new Error(task.error || '任务失败');
          return task || reply;
        });
      }
      return reply;
    }

    api.config().then(function (data) { ctx.state.config = data; fill(); }).catch(function () { fill(); });
    fill();
    fillProxy();
    fillCoreInfo();

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
          return fillProxy();
        })
        .catch(function (error) {
          sw.classList.toggle('is-on', on);
          ui.toast((action === 'start' ? '开启失败: ' : '关闭失败: ') + error.message, 'error');
        })
        .then(function () { sw.dataset.busy = ''; });
    });
    /* Core switch changes the running kernel (requires the binary installed).
     * It is a dedicated action, not part of the generic "save settings". */
    controls.coreType.addEventListener('change', function () {
      const next = controls.coreType.value;
      const prev = (ctx.state.config && ctx.state.config.coreType) || status.coreType || 'mihomo';
      if (next === prev) return;
      controls.coreType.disabled = true;
      ctx.api.coreSwitch(next)
        .then(function () {
          ctx.state.status = ctx.state.status || {};
          ctx.state.status.coreType = next;
          // Clear the stale version; refreshStatus() below repopulates the real
          // one from the core so we never show a hard-coded wrong version.
          ctx.state.status.coreVersion = '';
          ctx.state.config = ctx.state.config || {};
          ctx.state.config.coreType = next;
          ui.toast('已切换到 ' + (next === 'sing-box' ? 'sing-box' : 'mihomo') + ' 内核', 'ok');
          fill();
          fillCoreInfo();
          return ctx.refreshStatus();
        })
        .catch(function (error) {
          controls.coreType.value = prev;
          ui.toast('切换失败: ' + error.message, 'error');
        })
        .then(function () { controls.coreType.disabled = false; });
    });

    /* Online core update: start the backend updater and poll its state,
     * showing the live log + progress until it reaches done/failed. */
    const updateBtn = ui.$('[data-act="core-update"]', root);
    if (updateBtn) updateBtn.addEventListener('click', function () {
      const core = controls.coreType.value;
      updateBtn.disabled = true;
      const progress = ui.$('[data-role="update-progress"]', root);
      const bar = ui.$('[data-role="update-bar-fill"]', root);
      const log = ui.$('[data-role="update-log"]', root);
      progress.hidden = false;
      if (bar) bar.style.width = '15%';
      if (log) log.textContent = '正在连接更新源…';
      ctx.api.coreUpdate(core)
        .then(function () {
          ui.toast('开始更新 ' + (core === 'sing-box' ? 'sing-box' : 'mihomo') + ' 内核', 'ok');
          return new Promise(function (resolve) {
            let settled = false;
            function finish(info) {
              if (settled) return;
              settled = true;
              ui.stop('core-update-poll');
              resolve(info);
            }
            ui.every('core-update-poll', function () {
              ctx.api.coreUpdateStatus().then(function (info) {
                if (log && info.log) log.textContent = info.log;
                if (bar) bar.style.width = info.state === 'done' ? '100%' : '55%';
                if (info.state === 'done' || info.state === 'failed') finish(info);
              }).catch(function () {});
            }, 1500);
            // Safety net: never leave the button disabled if polling is torn
            // down by navigation before the backend reports completion.
            window.setTimeout(function () { finish({ state: 'failed', log: '更新状态轮询超时' }); }, 120000);
          });
        })
        .then(function (info) {
          if (info.state === 'done') {
            ui.toast('内核更新完成', 'ok');
            return ctx.refreshStatus().then(fill);
          }
          ui.toast('内核更新失败,请查看日志', 'error');
        })
        .catch(function (error) { ui.toast('更新失败: ' + error.message, 'error'); })
        .then(function () { updateBtn.disabled = false; });
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
        .then(settleTask)
        .then(function () { ui.toast(running ? '内核已停止' : '内核已启动', 'ok'); return ctx.refreshStatus(); })
        .then(fill)
        .catch(function (error) { ui.toast('操作失败: ' + error.message, 'error'); });
    });

    ui.$('[data-act="restart"]', root).addEventListener('click', function () {
      ctx.api.service('restart')
        .then(settleTask)
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
        tunStack: controls.tunStack.value,
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
