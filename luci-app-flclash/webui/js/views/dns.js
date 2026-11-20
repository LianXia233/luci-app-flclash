/* View: DNS 查询 — live mihomo DNS query log.
 *
 * Data source: the backend distils the core's debug log ([DNS] resolve /
 * cache hit / answer lines) into GET /dns/queries while this page polls it.
 * The core is temporarily switched to log-level=debug while the page is open
 * and restored afterwards — passive logging without flooding syslog.
 * A manual resolver is kept in the toolbar: those queries go through the
 * core resolver too, so they show up in the same log.
 */
(function () {
  'use strict';

  const ui = window.FlClashUI;
  const icon = window.FlClashIcons.svg;

  const TABS = ['全部', 'A', 'AAAA', '代理', '其他', '直连', '缓存'];

  function dnsQuery(ctx) {
    const api = ctx.api;
    const root = document.createElement('div');
    root.className = 'view page';
    let tab = '全部';
    let keyword = '';
    let rows = [];

    root.innerHTML =
      '<style>' +
        '.dns-domain{max-width:280px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}' +
        '.dns-result{word-break:break-all;max-width:420px}' +
        '.dns-source{max-width:230px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}' +
        '.dns-note{font-size:12px;opacity:.6;margin:8px 2px}' +
      '</style>' +
      '<div class="page-toolbar">' +
        '<div class="search">' + icon('search', 16) +
          '<input class="input" type="search" placeholder="过滤域名 / IP / 上游" aria-label="过滤 DNS 查询">' +
        '</div>' +
        '<input class="input" data-role="resolve-name" type="text" placeholder="手动查询域名" style="max-width:230px">' +
        '<select class="select" data-role="resolve-type" aria-label="记录类型">' +
          '<option value="A">A</option><option value="AAAA">AAAA</option>' +
          '<option value="CNAME">CNAME</option><option value="TXT">TXT</option></select>' +
        '<button class="btn is-primary" data-act="resolve" type="button">' + icon('bolt', 16) + '查询</button>' +
      '</div>' +
      '<div class="dns-note">查看本页时内核会临时开启 DNS 查询日志（debug），离开页面自动恢复；「发起」= 该解析使用的出站路径。</div>' +
      '<div class="seg-chips" data-role="tabs" role="tablist" aria-label="过滤"></div>' +
      '<section class="card is-flush"><div data-role="table"></div></section>';

    const search = ui.$('[type="search"]', root);
    const tabsBar = ui.$('[data-role="tabs"]', root);
    const table = ui.$('[data-role="table"]', root);

    function rowsOf() {
      return rows.filter(function (row) {
        if (tab === 'A' || tab === 'AAAA') {
          if ((row.qtype || '') !== tab) return false;
        } else if (tab === '缓存') {
          if (!row.cached) return false;
        } else if (tab !== '全部') {
          if ((row.origin || '') !== tab) return false;
        }
        if (!keyword) return true;
        const hay = ((row.domain || '') + ' ' + (row.result || '') + ' ' + (row.source || '')).toLowerCase();
        return hay.indexOf(keyword.toLowerCase()) !== -1;
      });
    }

    function renderTabs() {
      tabsBar.innerHTML = TABS.map(function (label) {
        return '<button class="seg-chip' + (tab === label ? ' is-active' : '') +
          '" data-tab="' + label + '" type="button" role="tab" aria-selected="' + (tab === label) + '">' +
          label + '</button>';
      }).join('');
      ui.$$('[data-tab]', tabsBar).forEach(function (button) {
        button.addEventListener('click', function () {
          tab = button.getAttribute('data-tab');
          renderTabs();
          renderRows();
        });
      });
    }

    function renderRows() {
      const list = rowsOf();
      if (!list.length) {
        table.innerHTML = ui.empty('暂无 DNS 查询记录 — 打开本页后内核会开始记录', 'dns');
        return;
      }
      table.innerHTML = '<table class="data"><thead><tr>' +
          '<th>时间</th><th>域名</th><th>类型</th><th>发起</th><th>结果</th><th>来源</th><th class="num">耗时</th>' +
        '</tr></thead><tbody>' +
        list.map(function (row) {
          const cached = row.cached ? '<span class="chip">缓存</span> ' : '';
          const result = row.result ? ui.esc(row.result) : '—';
          return '<tr>' +
            '<td class="muted">' + ui.esc(ui.clock(row.time)) + '</td>' +
            '<td class="dns-domain" title="' + ui.esc(row.domain) + '">' + ui.esc(row.domain) + '</td>' +
            '<td><span class="chip chip-plain">' + ui.esc(row.qtype || '—') + '</span></td>' +
            '<td><span class="chip chip-plain">' + ui.esc(row.origin || '其他') + '</span></td>' +
            '<td class="dns-result">' + cached + result + '</td>' +
            '<td class="muted dns-source" title="' + ui.esc(row.source || '') + '">' + ui.esc(row.source || '—') + '</td>' +
            '<td class="num">' + (row.elapsed || 0) + ' ms</td>' +
          '</tr>';
        }).join('') + '</tbody></table>';
    }

    function refresh() {
      return api.dnsQueries().then(function (data) {
        rows = Array.isArray(data) ? data : [];
        renderRows();
      }).catch(function (error) {
        if (error && error.status === 401) throw error;
        /* offline: keep the last rows on screen */
      });
    }

    search.addEventListener('input', ui.debounce(function () {
      keyword = search.value.trim();
      renderRows();
    }, 150));

    ui.$('[data-act="resolve"]', root).addEventListener('click', function () {
      const button = this;
      const input = ui.$('[data-role="resolve-name"]', root);
      const typeSel = ui.$('[data-role="resolve-type"]', root);
      const name = (input.value || '').trim();
      if (!name) { ui.toast('请输入要查询的域名', 'error'); return; }
      button.disabled = true;
      api.dnsResolve(name, typeSel.value)
        .then(function (reply) {
          const answers = ((reply && (reply.answer || reply.Answer)) || [])
            .map(function (record) { return record.data; });
          ui.toast(name + ' → ' + (answers.length ? answers.join(', ') : '无记录'), answers.length ? 'ok' : 'error');
          return refresh();
        })
        .catch(function (error) { ui.toast('查询失败: ' + (error.message || error), 'error'); })
        .then(function () { button.disabled = false; });
    });

    renderTabs();
    renderRows();
    refresh();
    const timer = window.setInterval(function () {
      if (document.hidden) return;
      refresh();
    }, 2000);
    if (ctx.onTeardown) ctx.onTeardown(function () { window.clearInterval(timer); });
    return root;
  }

  window.FlClashViews = window.FlClashViews || {};
  window.FlClashViews.dns = dnsQuery;
})();
