'use strict';
'require view';
'require fs';
'require uci';
'require ui';

/*
 * FlClash launcher.
 *
 * LuCI owns exactly two things here: starting the service and handing the
 * browser over to the WebUI. Every device operation lives in the WebUI.
 */
return view.extend({
	load: function () {
		return uci.load('flclash').catch(function () { /* defaults below */ });
	},

	readOption: function (name, fallback) {
		var value = uci.get('flclash', 'main', name);
		return (value === null || value === undefined || value === '') ? fallback : value;
	},

	targetUrl: function () {
		var path = String(this.readOption('webui_path', '/flclash/'));
		if (path.charAt(0) !== '/')
			path = '/' + path;
		if (path.charAt(path.length - 1) !== '/')
			path += '/';

		var port = parseInt(this.readOption('webui_port', '80'), 10) || 80;
		if (port === 80 || port === 443)
			return path;

		var host = window.location.hostname || 'localhost';
		var scheme = (port === 443 || window.location.protocol === 'https:') ? 'https' : 'http';
		return scheme + '://' + host + ':' + port + path;
	},

	startService: function () {
		if (this.readOption('enabled', '1') === '0')
			return Promise.reject(new Error('服务已在配置中禁用'));

		return fs.exec('/etc/init.d/flclash', ['start']).then(function (result) {
			/* procd reports the command's own status; a running instance exits 0. */
			if (result && typeof result.code === 'number' && result.code !== 0)
				throw new Error((result.stderr || result.stdout || '').trim() || ('启动失败，退出码 ' + result.code));
			return result;
		});
	},

	render: function () {
		var self = this;
		var target = this.targetUrl();

		var status = E('p', { 'class': 'cbi-value-description' }, [ _('正在启动 FlClash 服务并跳转到 WebUI…') ]);
		var action = E('button', {
			'class': 'cbi-button cbi-button-apply',
			'click': function (ev) {
				ev.preventDefault();
				self.launch(target, status, action);
			}
		}, [ _('重新启动并进入') ]);

		var card = E('div', { 'class': 'cbi-section' }, [
			E('h2', {}, [ _('FlClash') ]),
			E('div', { 'class': 'cbi-section-node' }, [
				status,
				E('p', { 'class': 'cbi-value-description' }, [
					E('a', { 'href': target }, [ target ])
				]),
				E('div', { 'class': 'right' }, [ action ])
			])
		]);

		window.setTimeout(function () { self.launch(target, status, action); }, 60);
		return card;
	},

	launch: function (target, status, action) {
		if (action) action.disabled = true;
		if (status) status.textContent = _('正在启动 FlClash 服务…');

		return this.startService().then(function () {
			if (status) status.textContent = _('服务已启动，正在进入 WebUI…');
			/* Give procd and the Go listener a moment before the browser asks. */
			window.setTimeout(function () { window.location.replace(target); }, 700);
		}).catch(function (error) {
			if (status) status.textContent = _('启动失败：') + (error && error.message ? error.message : error);
			if (action) action.disabled = false;
			ui.addNotification(null, E('p', {}, [
				_('无法启动 FlClash 服务。请检查 /etc/config/flclash 与系统日志。')
			]), 'danger');
		});
	},

	handleSaveApply: null,
	handleSave: null,
	handleReset: null
});
