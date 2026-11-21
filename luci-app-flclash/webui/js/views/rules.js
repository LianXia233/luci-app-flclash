/* View: 规则 — live rule list from the core with search. */
(function () {
  'use strict';

  const ui = window.FlClashUI;
  const icon = window.FlClashIcons.svg;

  function rules(ctx) {
    const api = ctx.api;
    const root = document.createElement('div');
    root.className = 'view page';
    let keyword = '';
    let all = [];

    root.innerHTML =
      '<div class="page-toolbar">' +
        '<div class="search">' + icon('search', 16) + '<input class="input" type="search" placeholder="筛选规则或出口" aria-label="筛选规则"></div>' +
        '<button class="btn" data-act="reload" type="button">' + icon('refresh', 16) + '刷新</button>' +
      '</div>' +
      '<section class="card" data-role="providers-card" hidden>' +
        '<div class="card-head"><span class="glyph" data-icon="folder"></span><span class="card-title">规则提供者</span>' +
        '<span class="card-end"><span class="chip" data-role="pcount">0</span></span></div>' +
        '<div data-role="providers"></div>' +
      '</section>' +
      '<section class="card is-flush">' +
        '<div class="card-head"><span class="glyph" data-icon="split"></span><span class="card-title">规则</span>' +
        '<span class="card-end"><span class="chip is-primary" data-role="count">0</span></span></div>' +
        '<div class="table-wrap" data-role="table">' + ui.skeletonRows(8) + '</div>' +
      '</section>';

    const tableWrap = ui.$('[data-role="table"]', root);
    const countChip = ui.$('[data-role="count"]', root);

    function render() {
      const data = all.filter(function (rule) {
        if (!keyword) return true;
        return (rule.payload + ' ' + rule.proxy + ' ' + rule.type).toLowerCase().indexOf(keyword) !== -1;
      });
      countChip.textContent = all.length + (keyword ? ' / ' + data.length : '');
      if (!data.length) {
        tableWrap.innerHTML = ui.empty(all.length ? '没有匹配的规则' : '暂无规则(内核停止或未加载配置)', 'split');
        return;
      }
      tableWrap.innerHTML = '<table class="data"><thead><tr><th>类型</th><th>规则</th><th>出口</th></tr></thead><tbody>' +
        data.slice(0, 300).map(function (rule) {
          return '<tr><td class="is-muted">' + ui.esc(rule.type || '-') + '</td>' +
            '<td>' + ui.esc(rule.payload || '-') + '</td>' +
            '<td><span class="chip chip-plain">' + ui.esc(rule.proxy || '-') + '</span></td></tr>';
        }).join('') + '</tbody></table>' +
        (data.length > 300 ? '<div class="table-more">仅显示前 300 条,使用搜索缩小范围</div>' : '');
    }

    function load() {
      api.rules().then(function (data) {
        all = (data && data.rules) || [];
        render();
      }).catch(function (error) {
        all = [];
        render();
      });
    }

    ui.$('input', root).addEventListener('input', ui.debounce(function (event) {
      keyword = event.target.value.trim().toLowerCase();
      render();
    }, 150));

    ui.$('[data-act="reload"]', root).addEventListener('click', function () {
      load();
      ui.toast('规则已刷新');
    });

    // Rule providers (mihomo). sing-box has no such API — the card stays
    // hidden rather than showing fake controls.
    const providersCard = ui.$('[data-role="providers-card"]', root);
    const providersBody = ui.$('[data-role="providers"]', root);
    const pcount = ui.$('[data-role="pcount"]', root);
    function loadProviders() {
      api.ruleProviders().then(function (data) {
        const items = (data && data.providers) || [];
        if (!items.length) { providersCard.hidden = true; return; }
        providersCard.hidden = false;
        pcount.textContent = String(items.length);
        providersBody.innerHTML = items.map(function (item) {
          const remote = item.vehicleType === 'HTTP';
          return '<div class="settings-row"><div class="row-copy">' +
            '<div class="row-title is-file">' + ui.esc(item.name) + '</div>' +
            '<div class="row-desc">' + ui.esc(item.behavior || '-') + ' · ' + ui.esc(item.ruleCount) + ' 条' +
            (item.updatedAt ? ' · ' + ui.esc(item.updatedAt.slice(0, 10)) : '') + '</div></div>' +
            (remote
              ? '<button class="btn" data-pupdate="' + ui.esc(item.name) + '" type="button">' + icon('refresh', 15) + '更新</button>'
              : '<span class="chip chip-plain">本地文件</span>') +
            '</div>';
        }).join('');
        ui.$$('[data-pupdate]', providersBody).forEach(function (button) {
          button.addEventListener('click', function () {
            button.disabled = true;
            api.updateRuleProvider(button.getAttribute('data-pupdate'))
              .then(function () { ui.toast('规则提供者已更新', 'ok'); loadProviders(); load(); })
              .catch(function (error) { ui.toast('更新失败: ' + error.message, 'error'); })
              .then(function () { button.disabled = false; });
          });
        });
      }).catch(function () { providersCard.hidden = true; });
    }

    load();
    loadProviders();
    return root;
  }

  window.FlClashViews = window.FlClashViews || {};
  window.FlClashViews.rules = rules;
})();
