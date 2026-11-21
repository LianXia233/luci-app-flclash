/* View: 配置 — profiles with real sync/update, edit (url/interval), apply and
 * delete. The update button re-downloads the subscription and re-applies it
 * when active (desktop-client behaviour). */
(function () {
  'use strict';

  const ui = window.FlClashUI;
  const icon = window.FlClashIcons.svg;

  function profiles(ctx) {
    const api = ctx.api;
    const root = document.createElement('div');
    root.className = 'view page';
    root.innerHTML =
      '<div class="page-toolbar"><div class="grow muted" data-role="summary">—</div>' +
        '<button class="btn" data-act="update-all" type="button">全部更新</button>' +
        '<button class="btn" data-act="import-local" type="button">本地导入</button>' +
        '<button class="btn is-primary" data-act="import" type="button">' + icon('plus', 16) + '导入订阅</button></div>' +
      '<div class="profile-list" data-role="list"></div>';

    const list = ui.$('[data-role="list"]', root);
    const comm = ctx.comm;

    /* Mutating endpoints answer 202 {taskId}; wait for the pushed task result
     * instead of assuming the work finished when the HTTP call returns. */
    function settle(reply) {
      if (reply && reply.taskId && comm) {
        return comm.waitTask(reply.taskId).then(function (task) {
          if (task && task.state === 'failed') throw new Error(task.error || '任务失败');
          return task || reply;
        });
      }
      return reply;
    }

    function render() {
      const items = ctx.state.profiles || [];
      ui.$('[data-role="summary"]', root).textContent =
        items.length + ' 个配置 · ' + items.filter(function (item) { return item.active; }).length + ' 个使用中';
      if (!items.length) {
        list.innerHTML = ui.empty('还没有配置,导入订阅开始使用', 'profiles');
        return;
      }
      list.innerHTML = items.map(function (item) {
        const share = Number(item.usedShare) || 0;
        const isLocal = item.type === 'local' || item.type === 'file';
        return '<div class="profile-item' + (item.active ? ' is-active' : '') + '">' +
          '<div class="profile-main">' +
            '<div class="row"><span class="profile-name">' + ui.esc(item.name) + '</span>' +
              (item.active ? '<span class="chip is-ok"><i class="chip-dot"></i>使用中</span>' : '') +
              '<span class="chip">' + (isLocal ? '本地' : '订阅') + '</span>' +
              (item.interval >= 60 ? '<span class="chip chip-plain">自动 ' + Math.round(item.interval / 60) + ' 分钟</span>' : '') + '</div>' +
            '<div class="profile-meta"><span>更新于 ' + ui.esc(item.updatedAt || '—') + '</span>' +
              (item.expireDays ? '<span>剩余 ' + ui.esc(item.expireDays) + ' 天</span>' : '') +
              '<span>' + ui.esc(item.proxies || 0) + ' 节点 · ' + ui.esc(item.groups || 0) + ' 组 · ' + ui.esc(item.rules || 0) + ' 规则</span></div>' +
            '<div class="profile-usage"><div class="usage-row"><strong>' + (share * 100).toFixed(1).replace(/\.0$/, '') + '% 已用</strong>' +
              '<span>' + ui.esc(item.used || '0 B') + ' / ' + ui.esc(item.total || '0 B') + '</span></div>' +
              '<svg class="wave" viewBox="0 0 300 12" preserveAspectRatio="none">' + ui.wave(share, 300, 12) + '</svg></div>' +
          '</div>' +
          '<div class="profile-actions">' +
            (isLocal ? '' : '<button class="icon-button" data-update="' + ui.esc(item.id) + '" type="button" title="更新订阅">' + icon('sync', 16) + '</button>') +
            '<button class="icon-button" data-edit="' + ui.esc(item.id) + '" type="button" title="编辑">' + icon('edit', 16) + '</button>' +
            '<button class="icon-button" data-yaml="' + ui.esc(item.id) + '" type="button" title="编辑 YAML">' + icon('file', 16) + '</button>' +
            (item.active ? '' : '<button class="btn" data-apply="' + ui.esc(item.id) + '" type="button">启用</button>') +
            '<button class="icon-button is-danger" data-delete="' + ui.esc(item.id) + '" type="button" title="删除">' + icon('trash', 16) + '</button>' +
          '</div>' +
        '</div>';
      }).join('');

      ui.$$('[data-update]', list).forEach(function (button) {
        button.addEventListener('click', function () {
          const id = button.getAttribute('data-update');
          button.classList.add('is-spinning');
          api.updateProfile(id)
            .then(settle)
            .then(function () { ui.toast('订阅已更新', 'ok'); return ctx.reloadProfiles(); })
            .catch(function (error) { ui.toast('更新失败: ' + error.message, 'error'); })
            .then(function () { button.classList.remove('is-spinning'); });
        });
      });

      ui.$$('[data-edit]', list).forEach(function (button) {
        button.addEventListener('click', function () {
          const id = button.getAttribute('data-edit');
          const item = (ctx.state.profiles || []).find(function (p) { return p.id === id; }) || {};
          ui.openSheet('编辑配置',
            '<div class="field"><label>名称</label><input id="edit-name" class="input" type="text" value="' + ui.esc(item.name || '') + '"></div>' +
            '<div class="field"><label>订阅链接</label><input id="edit-url" class="input" type="url" value="' + ui.esc(item.url || '') + '" placeholder="https://…"></div>' +
            '<div class="field"><label>User-Agent（可选，机场按 UA 下发配置）</label><input id="edit-ua" class="input" type="text" value="' + ui.esc(item.userAgent || '') + '" placeholder="clash.meta/v1.19.0"></div>' +
            '<div class="field"><label>下载超时（秒）</label><input id="edit-timeout" class="input" type="number" min="5" max="300" value="' + (item.timeout || 45) + '"></div>' +
            '<div class="field"><label>失败重试次数</label><input id="edit-retry" class="input" type="number" min="0" max="5" value="' + (typeof item.retry === 'number' ? item.retry : 1) + '"></div>' +
            '<div class="field"><label>自动更新(分钟,0 关闭)</label><input id="edit-interval" class="input" type="number" min="0" step="5" value="' + Math.round((item.interval || 0) / 60) + '"></div>' +
            '<p class="sheet-note">自动更新按订阅间隔重新下载并转换;启用中的配置会随后自动重载内核。</p>' +
            '<div class="sheet-actions"><button class="btn" data-cancel type="button">取消</button>' +
            '<button class="btn is-primary" data-confirm type="button">保存</button></div>',
            function (body, close, helpers) {
              ui.$('[data-cancel]', body).addEventListener('click', close);
              ui.$('[data-confirm]', body).addEventListener('click', function () {
                const name = ui.$('#edit-name', body).value.trim();
                const url = ui.$('#edit-url', body).value.trim();
                const minutes = Number(ui.$('#edit-interval', body).value) || 0;
                const patch = {
                  name: name,
                  url: url,
                  interval: minutes * 60,
                  userAgent: ui.$('#edit-ua', body).value.trim(),
                  timeout: Number(ui.$('#edit-timeout', body).value) || 45,
                  retry: Number(ui.$('#edit-retry', body).value) || 0
                };
                api.editProfile(id, patch)
                  .then(function () { helpers.markSaved(); close(); ui.toast('配置已保存', 'ok'); return ctx.reloadProfiles(); })
                  .catch(function (error) { ui.toast('保存失败: ' + error.message, 'error'); });
              });
            });
        });
      });

      ui.$$('[data-yaml]', list).forEach(function (button) {
        button.addEventListener('click', function () {
          const id = button.getAttribute('data-yaml');
          const item = (ctx.state.profiles || []).find(function (p) { return p.id === id; }) || {};
          api.profileRaw(id).then(function (data) {
            ui.openSheet('编辑 YAML · ' + (item.name || id),
              '<textarea id="yaml-text" class="input dns-area editor-area" spellcheck="false" style="min-height:320px">' + ui.esc(data.text || '') + '</textarea>' +
              '<div data-role="yaml-report"></div>' +
              '<p class="sheet-note">保存前会做结构校验；启用中的配置保存后自动重新应用（含混入），旧内容保留为 .bak。</p>' +
              '<div class="sheet-actions"><button class="btn" data-cancel type="button">取消</button>' +
              '<button class="btn" data-check type="button">校验</button>' +
              '<button class="btn is-primary" data-confirm type="button">保存</button></div>',
              function (body, close, helpers) {
                const area = ui.$('#yaml-text', body);
                const report = ui.$('[data-role="yaml-report"]', body);
                ui.$('[data-cancel]', body).addEventListener('click', close);
                ui.$('[data-check]', body).addEventListener('click', function () {
                  api.validateConfig({ text: area.value }).then(function (result) {
                    const check = result.base || {};
                    report.innerHTML = (check.ok
                      ? '<p class="sheet-note" style="color:var(--ok)">校验通过</p>'
                      : '<p class="sheet-note" style="color:var(--bad)">' + ui.esc((check.errors || []).join('；')) + '</p>');
                  }).catch(function (error) { ui.toast('校验失败: ' + error.message, 'error'); });
                });
                ui.$('[data-confirm]', body).addEventListener('click', function () {
                  const confirm = ui.$('[data-confirm]', body);
                  confirm.disabled = true;
                  api.saveProfileRaw(id, area.value)
                    .then(settle)
                    .then(function () {
                      helpers.markSaved();
                      close();
                      ui.toast(item.active ? '已保存并重新应用' : '已保存', 'ok');
                      return ctx.reloadProfiles();
                    })
                    .catch(function (error) {
                      ui.toast('保存失败: ' + error.message, 'error');
                      confirm.disabled = false;
                    });
                });
              });
          }).catch(function (error) { ui.toast('读取失败: ' + error.message, 'error'); });
        });
      });

      ui.$$('[data-apply]', list).forEach(function (button) {
        button.addEventListener('click', function () {
          button.disabled = true;
          api.applyProfile(button.getAttribute('data-apply'))
            .then(settle)
            .then(function () { ui.toast('配置已启用,内核已加载', 'ok'); return ctx.reloadProfiles(); })
            .catch(function (error) { ui.toast('启用失败: ' + error.message, 'error'); button.disabled = false; });
        });
      });

      ui.$$('[data-delete]', list).forEach(function (button) {
        button.addEventListener('click', function () {
          const id = button.getAttribute('data-delete');
          const item = (ctx.state.profiles || []).find(function (p) { return p.id === id; }) || {};
          ui.confirmSheet('删除配置', '确定删除「' + (item.name || id) + '」吗?该操作不可撤销。', function () {
            api.deleteProfile(id)
              .then(function () { ui.toast('配置已删除'); return ctx.reloadProfiles(); })
              .catch(function (error) { ui.toast('删除失败: ' + error.message, 'error'); });
          });
        });
      });
    }

    function openImport() {
      ui.openSheet('导入订阅',
        '<div class="field"><label for="import-url">订阅链接 / 分享链接</label>' +
          '<input id="import-url" class="input" type="url" placeholder="https://… 或 ss:// vmess:// vless:// trojan://" autocomplete="off"></div>' +
        '<div class="field"><label for="import-ua">User-Agent（可选，机场按 UA 下发配置）</label>' +
          '<input id="import-ua" class="input" type="text" placeholder="clash.meta/v1.19.0" autocomplete="off"></div>' +
        '<p class="sheet-note">支持 Clash/mihomo YAML、base64 节点列表以及 ss:// ssr:// vmess:// vless:// trojan:// hysteria2:// tuic:// 链接,导入时自动转换为内核配置。</p>' +
        '<div class="sheet-actions"><button class="btn" data-cancel type="button">取消</button>' +
        '<button class="btn is-primary" data-confirm type="button">导入</button></div>',
        function (body, close, helpers) {
          const urlInput = ui.$('#import-url', body);
          const uaInput = ui.$('#import-ua', body);
          const submit = function () {
            const value = (urlInput.value || '').trim();
            if (!value) { ui.toast('请输入链接', 'error'); urlInput.focus(); return; }
            ui.$('[data-confirm]', body).disabled = true;
            api.importProfile(value, (uaInput.value || '').trim())
              .then(settle)
              .then(function () {
                helpers.markSaved();
                close();
                ui.toast('订阅已导入', 'ok');
                return ctx.reloadProfiles();
              })
              .catch(function (error) {
                ui.toast('导入失败: ' + error.message, 'error');
                ui.$('[data-confirm]', body).disabled = false;
              });
          };
          ui.$('[data-cancel]', body).addEventListener('click', close);
          ui.$('[data-confirm]', body).addEventListener('click', submit);
          urlInput.addEventListener('keydown', function (event) { if (event.key === 'Enter') submit(); });
        });
    }

    function openImportLocal() {
      ui.openSheet('本地导入 YAML',
        '<div class="field"><label for="local-name">名称</label>' +
          '<input id="local-name" class="input" type="text" placeholder="本地配置" autocomplete="off"></div>' +
        '<div class="field"><label for="local-text">YAML / 分享链接内容</label>' +
          '<textarea id="local-text" class="input dns-area editor-area" rows="12" spellcheck="false" placeholder="粘贴 Clash/mihomo YAML 或节点链接列表"></textarea></div>' +
        '<p class="sheet-note">内容仅保存在路由器本地,不会联网下载;导入后可在列表中启用。</p>' +
        '<div class="sheet-actions"><button class="btn" data-cancel type="button">取消</button>' +
        '<button class="btn is-primary" data-confirm type="button">导入</button></div>',
        function (body, close, helpers) {
          const nameInput = ui.$('#local-name', body);
          const textInput = ui.$('#local-text', body);
          ui.$('[data-cancel]', body).addEventListener('click', close);
          ui.$('[data-confirm]', body).addEventListener('click', function () {
            const text = (textInput.value || '').trim();
            if (!text) { ui.toast('请输入 YAML 内容', 'error'); textInput.focus(); return; }
            ui.$('[data-confirm]', body).disabled = true;
            api.importLocalProfile((nameInput.value || '').trim(), text)
              .then(settle)
              .then(function () {
                helpers.markSaved();
                close();
                ui.toast('本地配置已导入', 'ok');
                return ctx.reloadProfiles();
              })
              .catch(function (error) {
                ui.toast('导入失败: ' + error.message, 'error');
                ui.$('[data-confirm]', body).disabled = false;
              });
          });
        });
    }

    function updateAll(button) {
      button.disabled = true;
      button.classList.add('is-spinning');
      api.updateAllProfiles()
        .then(settle)
        .then(function (task) {
          const result = (task && task.result) || {};
          const failed = (result.results || []).filter(function (item) { return !item.ok; }).length;
          if (typeof result.updated === 'number') {
            ui.toast(failed
              ? '更新 ' + (result.updated - failed) + ' 个，失败 ' + failed + ' 个'
              : '已更新 ' + result.updated + ' 个订阅', failed ? 'error' : 'ok');
          } else {
            ui.toast('全部订阅已更新', 'ok');
          }
          return ctx.reloadProfiles();
        })
        .catch(function (error) { ui.toast('批量更新失败: ' + error.message, 'error'); })
        .then(function () { button.disabled = false; button.classList.remove('is-spinning'); });
    }

    ui.$('[data-act="import"]', root).addEventListener('click', openImport);
    ui.$('[data-act="import-local"]', root).addEventListener('click', openImportLocal);
    ui.$('[data-act="update-all"]', root).addEventListener('click', function () { updateAll(this); });
    api.profiles().then(function (data) { ctx.state.profiles = data; render(); }).catch(function (error) {
      list.innerHTML = ui.empty(error.message, 'info');
    });
    ctx.onProfiles = render;
    if (ctx.state.profiles) render();
    const off = ctx.watch('profiles', function () { render(); });
    if (ctx.onTeardown) ctx.onTeardown(off);
    return root;
  }

  window.FlClashViews = window.FlClashViews || {};
  window.FlClashViews.profiles = profiles;
})();
