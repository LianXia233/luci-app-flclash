/* View: DNS 进阶 — mihomo advanced dns: fields (fallback chain, per-role
 * nameservers, fallback-filter, nameserver-policy). File-backed via
 * GET /dns/config; saving goes through the same /config PUT as basic
 * settings, which reloads a live core. Basic settings never wipe these
 * fields (backend merges absent keys from the current file). */
(function () {
  'use strict';

  const ui = window.FlClashUI;
  const icon = window.FlClashIcons.svg;

  function dnsAdvanced(ctx) {
    const api = ctx.api;
    const root = document.createElement('div');
    root.className = 'view page';

    root.innerHTML =
      '<div class="page-toolbar">' +
        '<div class="grow muted" data-role="state">—</div>' +
        '<button class="btn is-primary" data-act="save" type="button">' + icon('check', 16) + '保存</button>' +
      '</div>' +
      '<section class="card">' +
        '<div class="card-head"><span class="glyph" data-icon="dns"></span><span class="card-title">服务器分组</span></div>' +
        '<div class="settings-row"><div class="row-copy"><div class="row-title">Fallback 服务器</div>' +
        '<div class="row-desc">主 nameserver 结果被污染/境外时改用这些；每行一个（DoH/DoT/UDP）</div></div></div>' +
        '<textarea class="input dns-area" data-role="fallback" rows="3" spellcheck="false" placeholder="https://1.1.1.1/dns-query&#10;tls://8.8.4.4"></textarea>' +
        '<div class="settings-row"><div class="row-copy"><div class="row-title">代理节点域名解析</div>' +
        '<div class="row-desc">proxy-server-nameserver：解析机场节点域名，避免循环依赖</div></div></div>' +
        '<textarea class="input dns-area" data-role="proxyNs" rows="2" spellcheck="false" placeholder="https://223.5.5.5/dns-query"></textarea>' +
        '<div class="settings-row"><div class="row-copy"><div class="row-title">直连域名解析</div>' +
        '<div class="row-desc">direct-nameserver：直连规则命中的域名用国内 DNS</div></div></div>' +
        '<textarea class="input dns-area" data-role="directNs" rows="2" spellcheck="false" placeholder="223.5.5.5"></textarea>' +
        '<div class="settings-row"><div class="row-copy"><div class="row-title">DNS 解析 IPv6</div>' +
        '<div class="row-desc">dns.ipv6：允许返回 AAAA 记录</div></div>' +
        '<button class="switch" role="switch" data-role="ipv6" type="button"><span class="thumb"></span></button></div>' +
      '</section>' +
      '<section class="card">' +
        '<div class="card-head"><span class="glyph" data-icon="split"></span><span class="card-title">Fallback 过滤</span></div>' +
        '<div class="settings-row"><div class="row-copy"><div class="row-title">GeoIP 判断</div>' +
        '<div class="row-desc">主解析结果非本国 IP 时启用 fallback</div></div>' +
        '<button class="switch" role="switch" data-role="geoip" type="button"><span class="thumb"></span></button></div>' +
        '<div class="settings-row"><div class="row-copy"><div class="row-title">GeoIP 国家码</div>' +
        '<div class="row-desc">默认 CN</div></div>' +
        '<input class="input" data-role="geoipCode" maxlength="4" placeholder="CN" style="width:80px"></div>' +
        '<div class="settings-row"><div class="row-copy"><div class="row-title">污染 IP 网段</div>' +
        '<div class="row-desc">ipcidr：命中即视为污染，每行一个 CIDR</div></div></div>' +
        '<textarea class="input dns-area" data-role="ipcidr" rows="2" spellcheck="false" placeholder="240.0.0.0/4"></textarea>' +
        '<div class="settings-row"><div class="row-copy"><div class="row-title">强制 Fallback 域名</div>' +
        '<div class="row-desc">domain：这些域名始终使用 fallback 结果</div></div></div>' +
        '<textarea class="input dns-area" data-role="fbDomain" rows="2" spellcheck="false" placeholder="+.google.com"></textarea>' +
      '</section>' +
      '<section class="card">' +
        '<div class="card-head"><span class="glyph" data-icon="tune"></span><span class="card-title">分域名策略</span></div>' +
        '<div class="settings-row"><div class="row-copy"><div class="row-title">nameserver-policy</div>' +
        '<div class="row-desc">每行「域名匹配 = 服务器」，多个服务器用逗号分隔；支持 +.example.com / geosite:cn</div></div></div>' +
        '<textarea class="input dns-area" data-role="policy" rows="4" spellcheck="false" placeholder="+.internal.example.com = 10.0.0.1&#10;geosite:cn = 223.5.5.5,119.29.29.29"></textarea>' +
        '<p class="sheet-note">保存写入 config.yaml 的 dns: 段；内核运行中会自动重载。基础设置页的修改不会清掉这里的进阶字段。</p>' +
      '</section>';

    const stateLine = ui.$('[data-role="state"]', root);
    const el = {
      fallback: ui.$('[data-role="fallback"]', root),
      proxyNs: ui.$('[data-role="proxyNs"]', root),
      directNs: ui.$('[data-role="directNs"]', root),
      ipv6: ui.$('[data-role="ipv6"]', root),
      geoip: ui.$('[data-role="geoip"]', root),
      geoipCode: ui.$('[data-role="geoipCode"]', root),
      ipcidr: ui.$('[data-role="ipcidr"]', root),
      fbDomain: ui.$('[data-role="fbDomain"]', root),
      policy: ui.$('[data-role="policy"]', root)
    };
    let baseDns = null;

    function lines(area) {
      return area.value.split('\n').map(function (s) { return s.trim(); }).filter(Boolean);
    }
    function bindSwitch(node) {
      node.addEventListener('click', function () { node.classList.toggle('is-on'); });
    }
    bindSwitch(el.ipv6);
    bindSwitch(el.geoip);

    api.dnsConfig().then(function (data) {
      const dns = data.dns || {};
      baseDns = dns;
      el.fallback.value = (dns.fallback || []).join('\n');
      el.proxyNs.value = (dns['proxy-server-nameserver'] || []).join('\n');
      el.directNs.value = (dns['direct-nameserver'] || []).join('\n');
      el.ipv6.classList.toggle('is-on', dns.ipv6 === true);
      const fb = dns['fallback-filter'] || {};
      el.geoip.classList.toggle('is-on', fb.geoip === true);
      el.geoipCode.value = fb['geoip-code'] || '';
      el.ipcidr.value = (fb.ipcidr || []).join('\n');
      el.fbDomain.value = (fb.domain || []).join('\n');
      const policy = dns['nameserver-policy'] || {};
      el.policy.value = Object.keys(policy).map(function (key) {
        const servers = policy[key];
        return key + ' = ' + (Array.isArray(servers) ? servers.join(',') : servers);
      }).join('\n');
      stateLine.textContent = data.running ? '内核运行中，保存后自动重载生效' : '内核未运行，保存后下次启动生效';
    }).catch(function (error) { stateLine.textContent = error.message; });

    ui.$('[data-act="save"]', root).addEventListener('click', function () {
      const button = this;
      const policy = {};
      let policyError = '';
      lines(el.policy).forEach(function (line) {
        const idx = line.indexOf('=');
        if (idx <= 0) { policyError = '策略行缺少「=」: ' + line; return; }
        const key = line.slice(0, idx).trim();
        const servers = line.slice(idx + 1).split(',').map(function (s) { return s.trim(); }).filter(Boolean);
        if (!key || !servers.length) { policyError = '策略行不完整: ' + line; return; }
        policy[key] = servers;
      });
      if (policyError) { ui.toast(policyError, 'error'); return; }
      const code = el.geoipCode.value.trim().toUpperCase();
      if (code && !/^[A-Z]{2}$/.test(code)) { ui.toast('国家码必须是两位字母，如 CN', 'error'); return; }
      // Keep the basic fields exactly as they are in the file; only the
      // advanced keys are owned by this page.
      const dns = Object.assign({}, baseDns || {}, {
        fallback: lines(el.fallback),
        'proxy-server-nameserver': lines(el.proxyNs),
        'direct-nameserver': lines(el.directNs),
        ipv6: el.ipv6.classList.contains('is-on'),
        'fallback-filter': {
          geoip: el.geoip.classList.contains('is-on'),
          'geoip-code': code || 'CN',
          ipcidr: lines(el.ipcidr),
          domain: lines(el.fbDomain)
        },
        'nameserver-policy': policy
      });
      button.disabled = true;
      api.saveConfig({ dns: dns })
        .then(function () {
          baseDns = dns;
          ui.toast('DNS 进阶配置已保存', 'ok');
        })
        .catch(function (error) { ui.toast('保存失败: ' + error.message, 'error'); })
        .then(function () { button.disabled = false; });
    });

    return root;
  }

  window.FlClashViews = window.FlClashViews || {};
  window.FlClashViews.dnsAdvanced = dnsAdvanced;
})();