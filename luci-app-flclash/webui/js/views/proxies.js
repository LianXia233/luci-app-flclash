/* View: 代理 — group tabs, node grid, single + group latency tests. */
(function () {
  'use strict';

  const ui = window.FlClashUI;
  const icon = window.FlClashIcons.svg;

  function proxies(ctx) {
    const api = ctx.api;
    const state = ctx.state;
    const root = document.createElement('div');
    root.className = 'view page';
    let activeGroup = '';
    let keyword = '';
    let sortByDelay = false;
    let testing = false;

    root.innerHTML =
      '<div class="page-toolbar">' +
        '<div class="search">' + icon('search', 16) + '<input class="input" type="search" placeholder="搜索节点" aria-label="搜索节点"></div>' +
        '<button class="btn" data-act="delay" type="button">' + icon('bolt', 16) + '测速</button>' +
        '<button class="btn is-ghost" data-act="sort" type="button">' + icon('swap', 16) + '按延迟</button>' +
      '</div>' +
      '<div class="seg-chips" data-role="groups"></div>' +
      '<div class="node-grid" data-role="nodes"></div>';

    const search = ui.$('input', root);
    const groupsBar = ui.$('[data-role="groups"]', root);
    const grid = ui.$('[data-role="nodes"]', root);

    function renderGroups() {
      const groups = (state.proxies && state.proxies.groups) || [];
      if (!groups.length) {
        groupsBar.innerHTML = ui.empty('内核停止或无策略组', 'layers');
        return;
      }
      const all = '<button class="seg-chip' + (activeGroup ? '' : ' is-active') + '" data-group="" type="button">全部</button>';
      groupsBar.innerHTML = all + groups.map(function (group) {
        return '<button class="seg-chip' + (activeGroup === group.name ? ' is-active' : '') + '" data-group="' + ui.esc(group.name) + '" type="button">' +
          '<span class="seg-chip-name">' + ui.esc(group.name) + '</span>' +
          '<span class="seg-chip-now">' + ui.esc(group.now || '') + '</span></button>';
      }).join('');
      ui.$$('[data-group]', groupsBar).forEach(function (button) {
        button.addEventListener('click', function () {
          activeGroup = button.getAttribute('data-group');
          renderGroups();
          renderNodes();
        });
      });
    }

    function nodesOf() {
      const data = state.proxies || { nodes: [] };
      let nodes = (data.nodes || []).filter(function (node) {
        if (activeGroup && (node.group !== activeGroup && (node.groups || []).indexOf(activeGroup) === -1)) return false;
        if (!keyword) return true;
        return (node.name + ' ' + (node.type || '')).toLowerCase().indexOf(keyword.toLowerCase()) !== -1;
      });
      if (sortByDelay) {
        nodes = nodes.slice().sort(function (a, b) {
          return (Number(a.delay) || 99999) - (Number(b.delay) || 99999);
        });
      }
      return nodes;
    }

    function nodeCard(node) {
      const delay = Number(node.delay) || 0;
      const tone = ui.delayTone(delay);
      const active = (state.proxies.groups || []).some(function (group) {
        return group.now === node.name && (!activeGroup || group.name === activeGroup);
      });
      return '<div class="node-card' + (active ? ' is-active' : '') + '" data-node="' + ui.esc(node.name) + '" data-group="' + ui.esc(node.group) + '" role="button" tabindex="0">' +
        '<div class="node-top"><span class="node-name">' + ui.esc(node.name) + '</span>' +
          '<span class="chip is-' + tone + '">' + (delay ? delay + ' ms' : '超时') + '</span></div>' +
        '<div class="node-meta"><span class="chip chip-plain">' + ui.esc(node.type || 'node') + '</span>' +
          '<button class="icon-button" data-test="' + ui.esc(node.name) + '" type="button" aria-label="测速">' + icon('bolt', 14) + '</button></div>' +
        '</div>';
    }

    function renderNodes() {
      const nodes = nodesOf();
      grid.innerHTML = nodes.length ? nodes.map(nodeCard).join('') : ui.empty('没有匹配的节点', 'proxies');
      ui.$$('[data-test]', grid).forEach(function (button) {
        button.addEventListener('click', function (event) {
          event.stopPropagation();
          button.disabled = true;
          api.testDelay(button.getAttribute('data-test'))
            .then(function (result) { ui.toast(result.name + ' · ' + (result.value || '超时') + (result.value ? ' ms' : ''), result.value ? 'ok' : 'error'); return ctx.reloadProxies(); })
            .catch(function (error) { ui.toast('测速失败: ' + error.message, 'error'); button.disabled = false; });
        });
      });
      ui.$$('.node-card', grid).forEach(function (node) {
        const activate = function () {
          const group = node.getAttribute('data-group');
          const name = node.getAttribute('data-node');
          api.selectProxy(group, name)
            .then(function () { ui.toast('已切换到 ' + name, 'ok'); return ctx.reloadProxies(); })
            .catch(function (error) { ui.toast('切换失败: ' + error.message, 'error'); });
        };
        ui.pressable(node, activate);
        node.addEventListener('keydown', function (event) {
          if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); activate(); }
        });
      });
    }

    search.addEventListener('input', ui.debounce(function () {
      keyword = search.value.trim();
      renderNodes();
    }, 150));

    ui.$('[data-act="delay"]', root).addEventListener('click', function () {
      const button = this;
      if (testing) return;
      const groups = (state.proxies && state.proxies.groups) || [];
      const target = activeGroup || (groups[0] && groups[0].name);
      if (!target) { ui.toast('没有可测速的策略组', 'error'); return; }
      testing = true;
      button.disabled = true;
      button.innerHTML = icon('refresh', 16) + '测速中…';
      api.testGroupDelay(target)
        .then(function (result) {
          const delays = result && typeof result === 'object' ? result : {};
          const data = state.proxies;
          if (data && data.nodes) {
            data.nodes.forEach(function (node) {
              if (delays[node.name]) node.delay = delays[node.name];
              else if (delays[node.name] === 0) node.delay = 0;
            });
          }
          const okCount = Object.keys(delays).filter(function (k) { return delays[k] > 0; }).length;
          ui.toast('测速完成: ' + okCount + '/' + Object.keys(delays).length + ' 节点可用', 'ok');
          renderNodes();
        })
        .catch(function (error) { ui.toast('测速失败: ' + error.message, 'error'); })
        .then(function () { testing = false; button.disabled = false; button.innerHTML = icon('bolt', 16) + '测速'; });
    });

    ui.$('[data-act="sort"]', root).addEventListener('click', function () {
      sortByDelay = !sortByDelay;
      this.classList.toggle('is-primary', sortByDelay);
      renderNodes();
    });

    renderGroups();
    renderNodes();
    ctx.onProxies = function () { renderGroups(); renderNodes(); };
    ui.every('proxies', function () {
      api.proxies().then(function (data) {
        state.proxies = data;
        renderGroups();
        renderNodes();
      }).catch(function () {});
    }, 5000);
    return root;
  }

  window.FlClashViews = window.FlClashViews || {};
  window.FlClashViews.proxies = proxies;
})();
