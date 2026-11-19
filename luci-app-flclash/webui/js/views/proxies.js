/* View: 代理 — group tabs, node grid, single + group latency tests.
 *
 * Manual switching contract (the reason "点击没反应" used to happen):
 *   the clicked node is applied to a *selector* group that actually contains
 *   it — the active group when it is a selector, otherwise the node's primary
 *   selector group. Auto groups (url-test / fallback / load-balance) are
 *   rendered as read-only and explain why instead of failing silently.
 */
(function () {
  'use strict';

  const ui = window.FlClashUI;
  const icon = window.FlClashIcons.svg;

  function selectableOf(group) {
    if (!group) return false;
    if (typeof group.selectable === 'boolean') return group.selectable;
    const type = String(group.type || '').toLowerCase();
    return type === 'selector' || type === 'select';
  }

  function proxies(ctx) {
    const api = ctx.api;
    const state = ctx.state;
    const root = document.createElement('div');
    root.className = 'view page';
    let activeGroup = '';
    let keyword = '';
    let sortByDelay = false;
    let testing = false;
    let busy = false;

    root.innerHTML =
      '<div class="page-toolbar">' +
        '<div class="search">' + icon('search', 16) +
          '<input class="input" type="search" placeholder="搜索节点 / 策略组" aria-label="搜索节点">' +
        '</div>' +
        '<button class="btn" data-act="delay" type="button">' + icon('bolt', 16) + '测速</button>' +
        '<button class="btn is-ghost" data-act="sort" type="button">' + icon('swap', 16) + '按延迟</button>' +
      '</div>' +
      '<div class="seg-chips" data-role="groups" role="tablist" aria-label="策略组"></div>' +
      '<div data-role="note"></div>' +
      '<div class="node-grid" data-role="nodes"></div>';

    const search = ui.$('input', root);
    const groupsBar = ui.$('[data-role="groups"]', root);
    const noteBox = ui.$('[data-role="note"]', root);
    const grid = ui.$('[data-role="nodes"]', root);

    function groupList() { return (state.proxies && state.proxies.groups) || []; }

    function groupInfo(name) {
      return groupList().filter(function (group) { return group.name === name; })[0] || null;
    }

    /** Group a click should apply to: active selector, else primary selector.
     * An active *auto* group never redirects the click to another group — it
     * is blocked with an explanation instead (see `activate`). */
    function targetGroup(node) {
      const groups = (node.groups && node.groups.length) ? node.groups : [node.group];
      if (activeGroup) return groups.indexOf(activeGroup) !== -1 ? activeGroup : '';
      const manual = groups.filter(function (name) { return selectableOf(groupInfo(name)); })[0];
      return manual || groups[0] || node.group || '';
    }

    function renderNote() {
      const group = groupInfo(activeGroup);
      if (activeGroup && group && !selectableOf(group)) {
        noteBox.innerHTML = '<div class="group-note">' + icon('info', 16) +
          '<span>「' + ui.esc(group.name) + '」由内核自动挑选节点（' + ui.esc(group.type || 'url-test') +
          '），不能手动切换。切到「节点选择」等手动策略组后再选择节点。</span></div>';
      } else if (activeGroup) {
        const count = (state.proxies && state.proxies.nodes || []).filter(function (node) {
          return (node.groups || [node.group]).indexOf(activeGroup) !== -1;
        }).length;
        noteBox.innerHTML = '<div class="group-note is-quiet">' + icon('layers', 16) +
          '<span>手动策略组 · 共 ' + count + ' 个节点，点击立即切换。</span></div>';
      } else {
        noteBox.innerHTML = '';
      }
      noteBox.hidden = !noteBox.innerHTML;
    }

    function renderGroups() {
      const groups = groupList();
      if (!groups.length) {
        groupsBar.innerHTML = ui.empty('内核停止或无策略组', 'layers');
        renderNote();
        return;
      }
      const all = '<button class="seg-chip' + (activeGroup ? '' : ' is-active') + '" data-group="" type="button">' +
        '<span class="seg-chip-name">全部</span>' +
        '<span class="seg-chip-now">' + groups.length + ' 组</span></button>';
      groupsBar.innerHTML = all + groups.map(function (group) {
        const auto = selectableOf(group) ? '' : '<i class="seg-auto">自动</i>';
        return '<button class="seg-chip' + (activeGroup === group.name ? ' is-active' : '') +
          '" data-group="' + ui.esc(group.name) + '" type="button" role="tab" aria-selected="' + (activeGroup === group.name) + '">' +
          '<span class="seg-chip-name">' + ui.esc(group.name) + '</span>' +
          '<span class="seg-chip-now">' + auto + ui.esc(group.now || (group.count + ' 节点')) + '</span></button>';
      }).join('');
      ui.$$('[data-group]', groupsBar).forEach(function (button) {
        button.addEventListener('click', function () {
          const next = button.getAttribute('data-group');
          if (next === activeGroup) return;
          activeGroup = next;
          renderGroups();
          renderNote();
          renderNodes();
        });
      });
    }

    function nodesOf() {
      const data = state.proxies || { nodes: [] };
      let nodes = (data.nodes || []).filter(function (node) {
        if (activeGroup && (node.group !== activeGroup && (node.groups || []).indexOf(activeGroup) === -1)) return false;
        if (!keyword) return true;
        const haystack = (node.name + ' ' + (node.type || '') + ' ' + (node.group || '')).toLowerCase();
        return haystack.indexOf(keyword.toLowerCase()) !== -1;
      });
      if (sortByDelay) {
        nodes = nodes.slice().sort(function (a, b) {
          const av = a.delay > 0 ? a.delay : 99999;
          const bv = b.delay > 0 ? b.delay : 99999;
          return av - bv;
        });
      }
      return nodes;
    }

    function delayChip(node) {
      const delay = Number(node.delay) || 0;
      if (node.tested === false) return '<span class="chip chip-plain">未测速</span>';
      if (!delay) return '<span class="chip is-bad">超时</span>';
      return '<span class="chip is-' + ui.delayTone(delay) + '">' + delay + ' ms</span>';
    }

    function nodeCard(node) {
      const manual = node.selectable !== false && (node.selectable === true ||
        (node.groups || [node.group]).some(function (name) { return selectableOf(groupInfo(name)); }));
      const active = groupList().some(function (group) {
        return group.now === node.name && (activeGroup ? group.name === activeGroup : selectableOf(group));
      });
      const group = targetGroup(node);
      const locked = activeGroup ? !selectableOf(groupInfo(activeGroup)) : !manual;
      return '<div class="node-card' + (active ? ' is-active' : '') + (locked ? ' is-locked' : '') +
        '" data-node="' + ui.esc(node.name) + '" data-group="' + ui.esc(group) + '" role="button" tabindex="0"' +
        ' aria-pressed="' + (active ? 'true' : 'false') + '">' +
        '<div class="node-top"><span class="node-name" title="' + ui.esc(node.name) + '">' + ui.esc(node.name) + '</span></div>' +
        '<div class="node-meta"><span class="chip chip-plain">' + ui.esc(node.type || 'node') + '</span>' +
          delayChip(node) +
          (activeGroup ? '' : '<span class="node-group" title="点击将切换到「' + ui.esc(group) + '」">' + ui.esc(group) + '</span>') +
          (node.isGroup ? '' : '<button class="icon-button" data-test="' + ui.esc(node.name) + '" type="button" aria-label="测试此节点延迟">' + icon('bolt', 14) + '</button>') +
        '</div></div>';
    }

    function renderNodes() {
      const nodes = nodesOf();
      const running = !state.proxies || state.proxies.running !== false;
      if (!nodes.length) {
        grid.innerHTML = ui.empty(running ? '没有匹配的节点' : '内核已停止，启动后查看节点', 'proxies');
        return;
      }
      grid.innerHTML = nodes.map(nodeCard).join('');

      ui.$$('[data-test]', grid).forEach(function (button) {
        button.addEventListener('click', function (event) {
          event.stopPropagation();
          button.disabled = true;
          const name = button.getAttribute('data-test');
          api.testDelay(name)
            .then(function (result) {
              const value = Number(result && result.value) || 0;
              ui.toast(name + ' · ' + (value ? value + ' ms' : '超时'), value ? 'ok' : 'error');
              updateNodeDelay(name, value, true);
            })
            .catch(function (error) { ui.toast('测速失败: ' + error.message, 'error'); })
            .then(function () { button.disabled = false; });
        });
      });

      ui.$$('.node-card', grid).forEach(function (node) {
        const activate = function () {
          const name = node.getAttribute('data-node');
          const group = node.getAttribute('data-group');
          const info = groupInfo(group);
          if (!group) {
            ui.toast(activeGroup ? '该节点不在「' + activeGroup + '」中' : '找不到可用策略组', 'error');
            return;
          }
          if (!selectableOf(info)) {
            ui.toast('「' + group + '」由内核自动选择节点，无法手动切换', 'error');
            return;
          }
          if (info.now === name) { ui.toast('已在「' + group + '」选择 ' + name, 'ok'); return; }
          if (busy) return;
          busy = true;
          node.classList.add('is-pending');
          api.selectProxy(group, name, { requestId: 'ui-' + Date.now() })
            .then(function (reply) {
              const applied = (reply && reply.group) || group;
              mergeGroupNow(applied, name);
              ui.toast('已在「' + applied + '」切换到 ' + name, 'ok');
              return ctx.reloadProxies();
            })
            .catch(function (error) {
              ui.toast('切换失败: ' + (error.message || error), 'error');
            })
            .then(function () {
              busy = false;
              node.classList.remove('is-pending');
            });
        };
        ui.pressable(node, activate);
        node.addEventListener('keydown', function (event) {
          if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); activate(); }
        });
      });
    }

    function mergeGroupNow(group, name) {
      if (!state.proxies) return;
      const groups = (state.proxies.groups || []).map(function (item) {
        return item.name === group ? Object.assign({}, item, { now: name }) : item;
      });
      state.proxies = Object.assign({}, state.proxies, { groups: groups });
      renderGroups();
      renderNodes();
    }

    function updateNodeDelay(name, value, tested) {
      if (!state.proxies || !state.proxies.nodes) return;
      const nodes = state.proxies.nodes.map(function (node) {
        return node.name === name ? Object.assign({}, node, { delay: value, tested: tested }) : node;
      });
      state.proxies = Object.assign({}, state.proxies, { nodes: nodes });
      renderNodes();
    }

    search.addEventListener('input', ui.debounce(function () {
      keyword = search.value.trim();
      renderNodes();
    }, 150));

    ui.$('[data-act="delay"]', root).addEventListener('click', function () {
      const button = this;
      if (testing) return;
      const groups = groupList();
      const target = activeGroup || (groups[0] && groups[0].name);
      if (!target) { ui.toast('没有可测速的策略组', 'error'); return; }
      testing = true;
      button.disabled = true;
      button.innerHTML = icon('refresh', 16) + '测速中…';
      api.testGroupDelay(target)
        .then(function (result) {
          const delays = result && typeof result === 'object' ? result : {};
          const names = Object.keys(delays);
          if (names.length) {
            const nodes = (state.proxies && state.proxies.nodes || []).map(function (node) {
              if (!(node.name in delays)) return node;
              return Object.assign({}, node, { delay: Number(delays[node.name]) || 0, tested: true });
            });
            if (state.proxies) state.proxies = Object.assign({}, state.proxies, { nodes: nodes });
          }
          const okCount = names.filter(function (key) { return delays[key] > 0; }).length;
          ui.toast('测速完成: ' + okCount + '/' + names.length + ' 节点可用', 'ok');
          renderNodes();
        })
        .catch(function (error) { ui.toast('测速失败: ' + error.message, 'error'); })
        .then(function () {
          testing = false;
          button.disabled = false;
          button.innerHTML = icon('bolt', 16) + '测速';
        });
    });

    ui.$('[data-act="sort"]', root).addEventListener('click', function () {
      sortByDelay = !sortByDelay;
      this.classList.toggle('is-primary', sortByDelay);
      renderNodes();
    });

    renderGroups();
    renderNote();
    renderNodes();

    const off = ctx.watch('proxies', function () {
      renderGroups();
      renderNote();
      renderNodes();
    });
    if (ctx.onTeardown) ctx.onTeardown(off);

    ctx.onProxies = function () { renderGroups(); renderNote(); renderNodes(); };
    return root;
  }

  window.FlClashViews = window.FlClashViews || {};
  window.FlClashViews.proxies = proxies;
})();
