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

	readOption: function (section, name, fallback) {
		var value = uci.get('flclash', section, name);
		return (value === null || value === undefined || value === '') ? fallback : value;
	},

	targetUrl: function () {
		var path = String(this.readOption('webui', 'webui_path', '/flclash/'));
		if (path.charAt(0) !== '/')
			path = '/' + path;
		if (path.charAt(path.length - 1) !== '/')
			path += '/';

		var port = parseInt(this.readOption('webui', 'webui_port', '9098'), 10) || 9098;
		if (port === 80 || port === 443)
			return path;

		var host = window.location.hostname || 'localhost';
		var scheme = (port === 443 || window.location.protocol === 'https:') ? 'https' : 'http';
		return scheme + '://' + host + ':' + port + path;
	},

	/*
	 * The core package installs /etc/init.d/flclash-core. A companion API
	 * bridge, when present, is started first so the WebUI finds a live
	 * backend as soon as the redirect lands.
	 */
	startService: function () {
		if (this.readOption('main', 'enabled', '1') === '0')
			return Promise.reject(new Error('服务已在配置中禁用'));

		/* The API bridge is optional: ignore failures so a missing or broken
		 * bridge never blocks the core from starting. */
		var api = fs.exec('/etc/init.d/flclash-api', ['start']).catch(function () { });

		var core = function () {
			return fs.exec('/etc/init.d/flclash-core', ['start']).then(function (result) {
				if (result && typeof result.code === 'number' && result.code !== 0)
					throw new Error((result.stderr || result.stdout || '').trim() ||
						('启动内核失败，退出码 ' + result.code));
			});
		};

		return api.then(core);
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
