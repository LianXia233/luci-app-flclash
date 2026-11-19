/* View: 访问控制 (ACL) — Nikki-style LAN client allow/block lists plus
 * destination CIDR / port exclusions. Persists to UCI and rebuilds live
 * nftables/iptables rules via the backend. In TUN mode the LAN ACL does not
 * apply (L3 takeover) — surfaced explicitly, never a fake switch. */
(function () {
  'use strict';

  const ui = window.FlClashUI;
  const icon = window.FlClashIcons.svg;

  function acl(ctx) {
    const api = ctx.api;
    const root = document.createElement('div');
    root.className = 'view page';

    root.innerHTML =
      '<div class="page-toolbar">' +
        '<div class="grow muted" data-role="state">—</div>' +
        '<button class="btn is-primary" data-act="save" type="button">' + icon('check', 16) + '保存并生效</button>' +
      '</div>' +
      '<section class="card">' +
        '<div class="card-head"><span class="glyph" data-icon="acl"></span><span class="card-title">代理范围</span>' +
        '<span class="card-end"><span class="chip" data-role="badge">—</span></span></div>' +
        '<div class="settings-row is-wrap"><div class="row-copy"><div class="row-title">客户端模式</div>' +
        '<div class="row-desc">全部代理 / 白名单（仅列表内走代理）/ 黑名单（列表内直连）</div></div>' +
        '<div class="seg-chips" data-role="mode">' +
          '<button class="seg-chip" data-mode="all" type="button">全部</button>' +
          '<button class="seg-chip" data-mode="allow" type="button">白名单</button>' +
          '<button class="seg-chip" data-mode="block" type="button">黑名单</button>' +
        '</div></div>' +
        '<div class="settings-row"><div class="row-copy"><div class="row-title">客户端 IP / 网段</div>' +
        '<div class="row-desc">每行一条，如 192.168.1.50 或 10.0.0.0/24</div></div></div>' +
        '<textarea class="input dns-area" data-role="ips" rows="3" spellcheck="false" placeholder="192.168.1.50"></textarea>' +
        '<div class="settings-row"><div class="row-copy"><div class="row-title">客户端 MAC</div>' +
        '<div class="row-desc">每行一条，如 aa:bb:cc:dd:ee:ff（同一二层网络内有效）</div></div></div>' +
        '<textarea class="input dns-area" data-role="macs" rows="2" spellcheck="false" placeholder="aa:bb:cc:dd:ee:ff"></textarea>' +
      '</section>' +
      '<section class="card">' +
        '<div class="card-head"><span class="glyph" data-icon="split"></span><span class="card-title">目标排除</span></div>' +
        '<div class="settings-row"><div class="row-copy"><div class="row-title">排除网段</div>' +
        '<div class="row-desc">发往这些网段的流量永不接管（私有网段已默认排除）</div></div></div>' +
        '<textarea class="input dns-area" data-role="cidrs" rows="2" spellcheck="false" placeholder="10.10.0.0/16"></textarea>' +
        '<div class="settings-row"><div class="row-copy"><div class="row-title">排除端口</div>' +
        '<div class="row-desc">目标端口，每行一条，如 25（SMTP）</div></div></div>' +
        '<textarea class="input dns-area" data-role="ports" rows="2" spellcheck="false" placeholder="25"></textarea>' +
      '</section>' +
      '<section class="card">' +
        '<div class="card-head"><span class="glyph" data-icon="settings"></span><span class="card-title">路由器自身流量</span></div>' +
        '<div class="settings-row"><div class="row-copy"><div class="row-title">接管本机出站</div>' +
        '<div class="row-desc">路由器自身发起的连接也走代理（仅 mihomo + REDIRECT 模式）</div></div>' +
        '<button class="switch" role="switch" data-role="takeover" type="button"><span class="thumb"></span></button></div>' +
        '<p class="sheet-note">ACL 仅作用于 REDIRECT/TPROXY 透明代理；TUN 模式为三层接管，按内核规则分流。' +
        '白名单为空会被拒绝保存，避免整个局域网意外直连。</p>' +
      '</section>';

    const stateLine = ui.$('[data-role="state"]', root);
    const badge = ui.$('[data-role="badge"]', root);
    const modeSeg = ui.$('[data-role="mode"]', root);
    const areas = {
      ips: ui.$('[data-role="ips"]', root),
      macs: ui.$('[data-role="macs"]', root),
      cidrs: ui.$('[data-role="cidrs"]', root),
      ports: ui.$('[data-role="ports"]', root)
    };
    const takeover = ui.$('[data-role="takeover"]', root);
    let mode = 'all';
    let saved = null;

    function lines(area) {
      return area.value.split('\n').map(function (s) { return s.trim(); }).filter(Boolean);
    }
    function setMode(next) {
      mode = next;
      ui.$$('.seg-chip', modeSeg).forEach(function (chip) {
        chip.classList.toggle('is-active', chip.getAttribute('data-mode') === mode);
      });
      badge.textContent = mode === 'all' ? '全部代理' : mode === 'allow' ? '白名单' : '黑名单';
      badge.className = 'chip ' + (mode === 'all' ? '' : 'is-ok');
    }
    ui.$$('.seg-chip', modeSeg).forEach(function (chip) {
      chip.addEventListener('click', function () { setMode(chip.getAttribute('data-mode')); });
    });
    takeover.addEventListener('click', function () {
      takeover.classList.toggle('is-on');
      takeover.setAttribute('aria-checked', takeover.classList.contains('is-on') ? 'true' : 'false');
    });

    function paint(data) {
      const acl = data.acl || {};
      setMode(acl.mode || 'all');
      areas.ips.value = (acl.ips || []).join('\n');
      areas.macs.value = (acl.macs || []).join('\n');
      areas.cidrs.value = (acl.bypassCidrs || []).join('\n');
      areas.ports.value = (acl.bypassPorts || []).join('\n');
      takeover.classList.toggle('is-on', acl.localTakeover !== false);
      saved = JSON.stringify(collect());
      stateLine.textContent = data.applies
        ? '透明代理运行中，保存后立即重建防火墙规则'
        : (data.note || '透明代理未运行，保存后在下次开启时生效');
    }
    function collect() {
      return {
        mode: mode,
        ips: lines(areas.ips),
        macs: lines(areas.macs),
        bypassCidrs: lines(areas.cidrs),
        bypassPorts: lines(areas.ports),
        localTakeover: takeover.classList.contains('is-on')
      };
    }

    api.acl().then(paint).catch(function (error) { stateLine.textContent = error.message; });

    ui.$('[data-act="save"]', root).addEventListener('click', function () {
      const button = this;
      const payload = collect();
      if (payload.mode === 'allow' && !payload.ips.length && !payload.macs.length) {
        ui.toast('白名单模式下请先填写至少一个客户端 IP 或 MAC', 'error');
        return;
      }
      button.disabled = true;
      api.saveAcl(payload)
        .then(function (result) {
          saved = JSON.stringify(payload);
          ui.toast(result.reapplied ? '已保存并重建代理规则' : '已保存（代理未运行时下次开启生效）', 'ok');
          return api.acl().then(paint);
        })
        .catch(function (error) { ui.toast('保存失败: ' + error.message, 'error'); })
        .then(function () { button.disabled = false; });
    });

    return root;
  }

  window.FlClashViews = window.FlClashViews || {};
  window.FlClashViews.acl = acl;
})();