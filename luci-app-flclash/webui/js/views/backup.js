/* View: 备份与恢复 — real tar.gz snapshots of config.yaml / index.json /
 * profiles / mixin, download, upload and restore. Restore is a background
 * task: the sheet stays open with progress until the core reload finishes. */
(function () {
  'use strict';

  const ui = window.FlClashUI;
  const icon = window.FlClashIcons.svg;

  function backup(ctx) {
    const api = ctx.api;
    const comm = ctx.comm;
    const root = document.createElement('div');
    root.className = 'view page';
    let busy = false;

    root.innerHTML =
      '<div class="page-toolbar">' +
        '<div class="grow muted" data-role="summary">—</div>' +
        '<button class="btn" data-act="upload" type="button">' + icon('upload', 16) + '上传恢复</button>' +
        '<button class="btn is-primary" data-act="create" type="button">' + icon('backup', 16) + '创建备份</button>' +
        '<input type="file" accept=".gz,.tar.gz,application/gzip" hidden data-role="file">' +
      '</div>' +
      '<section class="card is-flush">' +
        '<div class="card-head"><span class="glyph" data-icon="backup"></span>' +
        '<span class="card-title">本机快照</span>' +
        '<span class="card-end"><button class="icon-button" data-act="reload" type="button" aria-label="刷新">' + icon('refresh', 15) + '</button></span></div>' +
        '<div data-role="list">' + ui.skeletonRows(3) + '</div>' +
      '</section>' +
      '<section class="card">' +
        '<div class="card-head"><span class="glyph" data-icon="info"></span><span class="card-title">备份范围</span></div>' +
        '<p class="sheet-note">包含 <code>config.yaml</code>（生成的内核配置）、<code>index.json</code>（订阅列表）、' +
        '<code>profiles/</code>（订阅正文）与 <code>mixin.yaml</code>（自定义混入）。' +
        '恢复前会自动保留一份 <code>pre-restore</code> 快照；恢复后内核自动重载。' +
        '透明代理与 LuCI 设置属于 UCI，随 OpenWrt 系统备份保存。</p>' +
      '</section>';

    const list = ui.$('[data-role="list"]', root);
    const fileInput = ui.$('[data-role="file"]', root);

    function settle(reply) {
      if (reply && reply.taskId && comm) {
        return comm.waitTask(reply.taskId).then(function (task) {
          if (task && task.state === 'failed') throw new Error(task.error || '任务失败');
          return task || reply;
        });
      }
      return reply;
    }

    function render(data) {
      const items = (data && data.snapshots) || [];
      ui.$('[data-role="summary"]', root).textContent =
        items.length + ' 个快照 · 保留最近 12 个';
      if (!items.length) {
        list.innerHTML = ui.empty('还没有备份，点右上角创建第一份快照', 'backup');
        return;
      }
      list.innerHTML = items.map(function (item) {
        const auto = item.name.indexOf('pre-restore-') === 0;
        return '<div class="settings-row">' +
          '<div class="row-copy"><div class="row-title is-file">' + ui.esc(item.name) +
            (auto ? ' <span class="chip chip-plain">恢复前自动</span>' : '') + '</div>' +
            '<div class="row-desc">' + ui.esc(ui.clock(item.createdAt)) + ' · ' + ui.bytes(item.size) + '</div></div>' +
          '<div class="row">' +
            '<button class="btn" data-down="' + ui.esc(item.name) + '" type="button">' + icon('down', 15) + '下载</button>' +
            '<button class="btn" data-restore="' + ui.esc(item.name) + '" type="button">' + icon('restore', 15) + '恢复</button>' +
            '<button class="icon-button is-danger" data-del="' + ui.esc(item.name) + '" type="button" aria-label="删除">' + icon('trash', 15) + '</button>' +
          '</div></div>';
      }).join('');

      ui.$$('[data-down]', list).forEach(function (button) {
        button.addEventListener('click', function () {
          const name = button.getAttribute('data-down');
          button.disabled = true;
          api.backupDownload(name).then(function (blob) {
            const href = URL.createObjectURL(blob);
            const a = document.createElement('a');
            a.href = href;
            a.download = name;
            document.body.appendChild(a);
            a.click();
            a.remove();
            window.setTimeout(function () { URL.revokeObjectURL(href); }, 4000);
          }).catch(function (error) { ui.toast(error.message, 'error'); })
            .then(function () { button.disabled = false; });
        });
      });
      ui.$$('[data-restore]', list).forEach(function (button) {
        button.addEventListener('click', function () {
          const name = button.getAttribute('data-restore');
          ui.confirmSheet('恢复备份', '用「' + name + '」覆盖当前配置与订阅？当前数据会先自动备份为 pre-restore 快照。', function () {
            run(function () { return api.backupRestore(name); }, '备份已恢复，内核已重载');
          }, true);
        });
      });
      ui.$$('[data-del]', list).forEach(function (button) {
        button.addEventListener('click', function () {
          const name = button.getAttribute('data-del');
          ui.confirmSheet('删除快照', '确定删除「' + name + '」吗？', function () {
            api.backupDelete(name)
              .then(function () { ui.toast('快照已删除'); return load(); })
              .catch(function (error) { ui.toast('删除失败: ' + error.message, 'error'); });
          }, true);
        });
      });
    }

    function load() {
      return api.backupList()
        .then(render)
        .catch(function (error) {
          list.innerHTML = ui.empty(error.message, 'info');
        });
    }

    function run(action, okMessage) {
      if (busy) return Promise.resolve();
      busy = true;
      const buttons = ui.$$('.page-toolbar .btn', root);
      buttons.forEach(function (button) { button.disabled = true; });
      ui.toast('正在处理…');
      return Promise.resolve()
        .then(action)
        .then(settle)
        .then(function () { ui.toast(okMessage, 'ok'); return load(); })
        .catch(function (error) { ui.toast('操作失败: ' + error.message, 'error'); })
        .then(function () {
          busy = false;
          buttons.forEach(function (button) { button.disabled = false; });
        });
    }

    ui.$('[data-act="create"]', root).addEventListener('click', function () {
      run(function () { return api.backupCreate('manual'); }, '备份已创建');
    });

    ui.$('[data-act="reload"]', root).addEventListener('click', function () { load(); });

    ui.$('[data-act="upload"]', root).addEventListener('click', function () { fileInput.click(); });
    fileInput.addEventListener('change', function () {
      const file = fileInput.files && fileInput.files[0];
      fileInput.value = '';
      if (!file) return;
      if (file.size > 24 * 1024 * 1024) { ui.toast('备份文件过大（上限 24MB）', 'error'); return; }
      ui.confirmSheet('上传并恢复', '用「' + file.name + '」覆盖当前配置与订阅？', function () {
        file.arrayBuffer().then(function (buffer) {
          run(function () { return api.backupUpload(buffer); }, '上传的备份已恢复');
        });
      }, true);
    });

    load();
    return root;
  }

  window.FlClashViews = window.FlClashViews || {};
  window.FlClashViews.backup = backup;
})();
