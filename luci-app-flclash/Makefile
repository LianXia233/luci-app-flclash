include $(TOPDIR)/rules.mk

PKG_NAME:=luci-app-flclash
PKG_VERSION:=1.4.0
PKG_RELEASE:=8

PKG_MAINTAINER:=FlClash WebUI
PKG_LICENSE:=GPL-3.0-only
PKG_LICENSE_FILES:=LICENSE

# ---------------------------------------------------------------------------
# 单包交付 (single-package delivery)
#
# ONE .apk installs the whole stack:
#   - Go core               /usr/bin/mihomo
#   - Rust IPC host         /usr/bin/flclash-core-bridge
#   - Rust/Tokio API bridge /usr/bin/flclash-api
#   - procd services, UCI defaults, LuCI entry, rpcd object, static WebUI
#
# Binary policy (Rust official SDK only):
#   mihomo / flclash-api / flclash-core-bridge are prebuilt OUTSIDE the OpenWrt
#   SDK, by their own official toolchains:
#     * Go:      CGO_ENABLED=0 GOOS=linux (with_gvisor, -ldflags "-w -s")
#     * Rust:    rustup + rust-std for {aarch64,x86_64}-unknown-linux-musl,
#                linked by rust-lld (-C link-self-contained=yes)
#   The OpenWrt SDK below is used ONLY to assemble/sign the .apk. Build/Compile
#   is intentionally empty: the OP SDK never compiles Rust (no OP rust package,
#   no cargo wrapper, no host Rust toolchain).
# ---------------------------------------------------------------------------

include $(INCLUDE_DIR)/package.mk

define Package/luci-app-flclash
  SECTION:=net
  CATEGORY:=Network
  SUBMENU:=Virtual Private Network (VPN)
  TITLE:=FlClash WebUI — mihomo/sing-box single package
  # Pure fw4 data plane. `curl` is required at runtime: the traffic/log
  # samplers stream the controller API with `curl -N`, and the online core
  # updater downloads releases with it. kmod-nft-nat/-tproxy/-tun are published
  # in the 25.12 kmods feed; ip-full provides `ip rule` / `ip -6 rule` and the
  # policy-routing tables (busybox ip is not sufficient).
  DEPENDS:=+luci-base +rpcd +rpcd-mod-ucode +ucode +curl \
           +firewall4 +kmod-nft-nat +kmod-nft-tproxy +kmod-tun +ip-full
  PKGARCH:=$(ARCH_PACKAGES)
endef

define Package/luci-app-flclash/description
  FlClash for OpenWrt / ImmortalWrt in one apk: static mihomo (FlClash
  native-IPC core), Rust/Tokio API bridge + IPC host, LuCI launcher and the
  HTML5 WebUI with mihomo/sing-box core switching.
endef

define Package/luci-app-flclash/conffiles
/etc/config/flclash
endef

define Build/Prepare
	mkdir -p $(PKG_BUILD_DIR)
endef

define Build/Compile
endef

define Package/luci-app-flclash/install
	# --- binaries: staged by CI from the official toolchains -----------------
	$(INSTALL_DIR) $(1)/usr/bin
	$(INSTALL_BIN) ./files/usr/bin/mihomo $(1)/usr/bin/mihomo
	$(INSTALL_BIN) ./files/usr/bin/flclash-core-bridge $(1)/usr/bin/flclash-core-bridge
	$(INSTALL_BIN) ./files/usr/bin/flclash-api $(1)/usr/bin/flclash-api

	# --- procd services + config ---------------------------------------------
	$(INSTALL_DIR) $(1)/etc/config $(1)/etc/init.d $(1)/etc/uci-defaults
	$(INSTALL_DATA) ./files/etc/config/flclash $(1)/etc/config/flclash
	$(INSTALL_BIN) ./files/etc/init.d/flclash-core $(1)/etc/init.d/flclash-core
	$(INSTALL_BIN) ./files/etc/init.d/flclash-api $(1)/etc/init.d/flclash-api
	$(INSTALL_BIN) ./files/etc/init.d/flclash-proxy $(1)/etc/init.d/flclash-proxy
	$(INSTALL_BIN) ./files/etc/init.d/flclash-watchdog $(1)/etc/init.d/flclash-watchdog
	$(INSTALL_BIN) ./files/etc/uci-defaults/99-flclash-webui $(1)/etc/uci-defaults/99-flclash-webui

	# --- helper scripts (singbox-convert.sh is required by the core init) -----
	$(INSTALL_DIR) $(1)/usr/libexec/flclash
	$(INSTALL_BIN) ./files/usr/libexec/flclash/core-update.sh $(1)/usr/libexec/flclash/core-update.sh
	$(INSTALL_BIN) ./files/usr/libexec/flclash/proxy-rules.sh $(1)/usr/libexec/flclash/proxy-rules.sh
	$(INSTALL_BIN) ./files/usr/libexec/flclash/stop-all.sh $(1)/usr/libexec/flclash/stop-all.sh
	$(INSTALL_BIN) ./files/usr/libexec/flclash/singbox-convert.sh $(1)/usr/libexec/flclash/singbox-convert.sh
	$(INSTALL_BIN) ./files/usr/libexec/flclash/watchdog.sh $(1)/usr/libexec/flclash/watchdog.sh

	# --- netifd hotplug: re-converge on WAN/LAN up/down -----------------------
	$(INSTALL_DIR) $(1)/etc/hotplug.d/iface
	$(INSTALL_BIN) ./files/etc/hotplug.d/iface/25-flclash $(1)/etc/hotplug.d/iface/25-flclash

	# --- sysupgrade keep list -------------------------------------------------
	$(INSTALL_DIR) $(1)/lib/upgrade/keep.d
	$(INSTALL_DATA) ./files/lib/upgrade/keep.d/luci-app-flclash $(1)/lib/upgrade/keep.d/luci-app-flclash

	# --- LuCI entry + rpcd object --------------------------------------------
	$(INSTALL_DIR) $(1)/usr/share/luci/menu.d
	$(INSTALL_DATA) ./files/usr/share/luci/menu.d/luci-app-flclash-webui.json $(1)/usr/share/luci/menu.d/luci-app-flclash-webui.json
	$(INSTALL_DIR) $(1)/usr/share/rpcd/acl.d
	$(INSTALL_DATA) ./files/usr/share/rpcd/acl.d/luci-app-flclash-webui.json $(1)/usr/share/rpcd/acl.d/luci-app-flclash-webui.json
	$(INSTALL_DIR) $(1)/usr/share/rpcd/ucode
	$(INSTALL_DATA) ./files/usr/share/rpcd/ucode/flclash.uc $(1)/usr/share/rpcd/ucode/flclash.uc
	$(INSTALL_DIR) $(1)/usr/share/ucode/flclash
	$(INSTALL_DATA) ./files/usr/share/ucode/flclash/fw4_input.ut $(1)/usr/share/ucode/flclash/fw4_input.ut

	$(INSTALL_DIR) $(1)/www/luci-static/resources/view/flclash
	$(INSTALL_DATA) ./files/www/luci-static/resources/view/flclash/launch.js $(1)/www/luci-static/resources/view/flclash/launch.js

	# --- static WebUI ---------------------------------------------------------
	$(INSTALL_DIR) $(1)/www/flclash/css
	$(INSTALL_DIR) $(1)/www/flclash/js/views
	$(INSTALL_DIR) $(1)/www/flclash/fonts
	$(INSTALL_DIR) $(1)/www/flclash/img
	$(INSTALL_DATA) ./webui/index.html $(1)/www/flclash/index.html
	$(INSTALL_DATA) ./webui/css/app.css $(1)/www/flclash/css/app.css
	$(INSTALL_DATA) ./webui/css/views.css $(1)/www/flclash/css/views.css
	$(INSTALL_DATA) ./webui/js/config.js $(1)/www/flclash/js/config.js
	$(INSTALL_DATA) ./webui/js/icons.js $(1)/www/flclash/js/icons.js
	$(INSTALL_DATA) ./webui/js/ui.js $(1)/www/flclash/js/ui.js
	$(INSTALL_DATA) ./webui/js/mock.js $(1)/www/flclash/js/mock.js
	$(INSTALL_DATA) ./webui/js/api.js $(1)/www/flclash/js/api.js
	$(INSTALL_DATA) ./webui/js/comm.js $(1)/www/flclash/js/comm.js
	$(INSTALL_DATA) ./webui/js/views.js $(1)/www/flclash/js/views.js
	$(INSTALL_DATA) ./webui/js/app.js $(1)/www/flclash/js/app.js
	$(INSTALL_DATA) ./webui/js/views/*.js $(1)/www/flclash/js/views/
	$(INSTALL_DATA) ./webui/fonts/material-symbols.woff2 $(1)/www/flclash/fonts/
	$(INSTALL_DATA) ./webui/img/icon.png $(1)/www/flclash/img/
endef

define Package/luci-app-flclash/postinst
#!/bin/sh
[ -n "$${IPKG_INSTROOT}" ] || {
	# The proxy core is deliberately NOT boot-managed (the WebUI owns its
	# switch, see README). Enabling it here on every upgrade silently
	# contradicted that after the first-boot uci-defaults had disabled it.
	/etc/init.d/flclash-api enable
	/etc/init.d/flclash-proxy enable
	/etc/init.d/flclash-watchdog enable
	# Credentials: keep the API secret in a 0600 root-only file, never in UCI
	# (UCI files are world-readable). Migrate the legacy UCI value once and
	# generate a secret when none exists, so `require_secret=1` can be a hard
	# startup requirement instead of an unauthenticated LAN control plane.
	umask 077
	mkdir -p /etc/flclash 2>/dev/null
	chmod 0700 /etc/flclash 2>/dev/null
	if [ -f /etc/config/flclash ]; then
		secret_file="$$(uci -q get flclash.api.secret_file)"
		[ -n "$$secret_file" ] || secret_file=/etc/flclash/api.secret
		legacy="$$(uci -q get flclash.api.secret)"
		secret=""
		[ -r "$$secret_file" ] && secret="$$(head -n 1 "$$secret_file" 2>/dev/null | tr -d '\r\n')"
		[ -z "$$secret" ] && [ -n "$$legacy" ] && secret="$$legacy"
		[ -z "$$secret" ] && secret="$$(head -c 32 /dev/urandom 2>/dev/null | md5sum 2>/dev/null | cut -d' ' -f1)"
		if [ -n "$$secret" ]; then
			printf '%s\n' "$$secret" > "$$secret_file"
			chmod 0600 "$$secret_file" 2>/dev/null
		fi
		uci -q delete flclash.api.secret 2>/dev/null
		uci -q set flclash.api.secret_file="$$secret_file"
		uci -q set flclash.api.require_secret='1'
		# Mode/ipv6 schema (see proxy-rules.sh). Derive once from legacy keys.
		if ! uci -q get flclash.proxy.mode >/dev/null 2>&1; then
			if [ "$$(uci -q get flclash.main.tun_enabled)" = "1" ]; then
				uci -q set flclash.proxy.mode='tun'
			elif [ "$$(uci -q get flclash.proxy.enabled)" = "1" ]; then
				uci -q set flclash.proxy.mode='redirect'
			else
				uci -q set flclash.proxy.mode='off'
			fi
		fi
		if ! uci -q get flclash.proxy.ipv6 >/dev/null 2>&1; then
			if [ "$$(uci -q get flclash.proxy.filter_aaaa)" = "1" ]; then
				uci -q set flclash.proxy.ipv6='off'
			else
				uci -q set flclash.proxy.ipv6='proxy'
			fi
		fi
		uci -q get flclash.proxy.quic >/dev/null 2>&1 || uci -q set flclash.proxy.quic='proxy'
		uci -q commit flclash 2>/dev/null
		chmod 0600 /etc/config/flclash 2>/dev/null
	fi
	# Persistent fw4 hook (see proxy-rules.sh): the include is replayed on
	# every firewall reload instead of relying on runtime nft inserts.
	if [ -f /etc/config/firewall ]; then
		inc_added=0
		if uci -q get firewall.flclash >/dev/null 2>&1; then
			inc=flclash
		else
			uci -q set firewall.flclash=include
			inc=flclash
			inc_added=1
		fi
		if [ -n "$$inc" ]; then
			uci -q set "firewall.$$inc.enabled='1'"
			uci -q set "firewall.$$inc.type='nftables'"
			uci -q set "firewall.$$inc.path='/var/run/flclash/fw4-input.nft'"
			uci -q set "firewall.$$inc.position='chain-prepend'"
			uci -q set "firewall.$$inc.chain='input'"
			uci -q commit firewall 2>/dev/null
			mkdir -p /var/run/flclash 2>/dev/null
			chmod 0700 /var/run/flclash 2>/dev/null
			[ -f /var/run/flclash/fw4-input.nft ] || : > /var/run/flclash/fw4-input.nft 2>/dev/null
			chmod 0600 /var/run/flclash/fw4-input.nft 2>/dev/null
			# On upgrade, rebuild fw4 input once so rules inserted by the old
			# runtime `nft insert` path are replaced by the include.
			[ "$$inc_added" = "1" ] && [ -x /etc/init.d/firewall ] && /etc/init.d/firewall reload >/dev/null 2>&1
		fi
	fi
}
exit 0
endef

define Package/luci-app-flclash/prerm
#!/bin/sh
[ -n "$${IPKG_INSTROOT}" ] || {
	# Stop the whole stack and remove every proxy artifact before the binaries
	# disappear. User data (/etc/flclash, /etc/config/flclash) is preserved.
	[ -x /usr/libexec/flclash/stop-all.sh ] && /usr/libexec/flclash/stop-all.sh >/dev/null 2>&1
	/etc/init.d/flclash-core stop >/dev/null 2>&1
	/etc/init.d/flclash-proxy stop >/dev/null 2>&1
	/etc/init.d/flclash-api stop >/dev/null 2>&1
	/etc/init.d/flclash-watchdog stop >/dev/null 2>&1
	/etc/init.d/flclash-core disable >/dev/null 2>&1
	/etc/init.d/flclash-proxy disable >/dev/null 2>&1
	/etc/init.d/flclash-api disable >/dev/null 2>&1
	/etc/init.d/flclash-watchdog disable >/dev/null 2>&1
	rm -rf /var/run/flclash /tmp/flclash-proxy.lock /tmp/flclash-jump.tokens
}
exit 0
endef

$(eval $(call BuildPackage,luci-app-flclash))
