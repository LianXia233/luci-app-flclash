/* Read-only system proxy information from the live device.
 * Exposed via rpcd ubus as flclash.system_proxy.
 * Deliberately returns NO credentials, subscription URLs, node addresses
 * or passwords — only non-sensitive proxy system state.
 */
'use strict';

import { uci } from 'uci';
import { popen } from 'fs';

function str(value, fallback) {
	return value == null || value === '' ? (fallback || '') : String(value);
}

function num(value, fallback) {
	const n = Number(value);
	return Number.isFinite(n) && n > 0 ? n : (fallback || 0);
}

function readUci() {
	const result = {
		backend: 'none',
		mixed_port: 0,
		clash_api_port: 0,
		tun_name: '',
		routing_mode: '',
		ipv6_support: false,
		running: false
	};

	// FlClash core (preferred target of this package)
	const flclash = uci.get_all('flclash') || {};
	const main = flclash.main || {};
	if (str(main.enabled, '0') !== '0') {
		result.backend = 'flclash';
		result.mixed_port = num(main.mixed_port, 0);
		result.clash_api_port = num(main.clash_api_port, 0);
		result.tun_name = str(main.tun_name, '');
		result.routing_mode = str(main.routing_mode, '');
		result.ipv6_support = str(main.ipv6, '0') === '1';
	}

	// homeproxy / sing-box (fallback: report the live system proxy)
	const hp = uci.get_all('homeproxy') || {};
	const infra = hp.infra || {};
	const config = hp.config || {};
	if (!result.backend && str(infra.mixed_port, '')) {
		result.backend = 'homeproxy';
		result.mixed_port = num(infra.mixed_port, 0);
		result.clash_api_port = num(infra.clash_api_port, 0);
		result.tun_name = str(infra.tun_name, '');
		result.routing_mode = str(config.routing_mode, '');
		result.ipv6_support = str(config.ipv6_support, '0') === '1';
	}

	return result;
}

function runningProcess() {
	const out = popen('ps w');
	if (!out) return false;
	return /sing-box|mihomo|clash/.test(out);
}

return {
	system_proxy: {
		call: function () {
			const info = readUci();
			info.running = runningProcess();
			info.read_only = true;
			info.source = 'device-uci-ps';
			return info;
		}
	}
};
