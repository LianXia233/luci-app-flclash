include $(TOPDIR)/rules.mk

PKG_NAME:=luci-app-flclash
PKG_VERSION:=1.3.0
PKG_RELEASE:=1

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
  # kernel modules are NOT published in the immortalwrt 25.12 apk feeds, so
  # they must not be hard dependencies (apk would fail to resolve them).
  DEPENDS:=+luci-base +rpcd +rpcd-mod-ucode
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
	$(INSTALL_BIN) ./files/etc/uci-defaults/99-flclash-webui $(1)/etc/uci-defaults/99-flclash-webui

	# --- helper scripts (singbox-convert.sh is required by the core init) -----
	$(INSTALL_DIR) $(1)/usr/libexec/flclash
	$(INSTALL_BIN) ./files/usr/libexec/flclash/core-update.sh $(1)/usr/libexec/flclash/core-update.sh
	$(INSTALL_BIN) ./files/usr/libexec/flclash/proxy-rules.sh $(1)/usr/libexec/flclash/proxy-rules.sh
	$(INSTALL_BIN) ./files/usr/libexec/flclash/singbox-convert.sh $(1)/usr/libexec/flclash/singbox-convert.sh

	# --- LuCI entry + rpcd object --------------------------------------------
	$(INSTALL_DIR) $(1)/usr/share/luci/menu.d
	$(INSTALL_DATA) ./files/usr/share/luci/menu.d/luci-app-flclash-webui.json $(1)/usr/share/luci/menu.d/luci-app-flclash-webui.json
	$(INSTALL_DIR) $(1)/usr/share/rpcd/acl.d
	$(INSTALL_DATA) ./files/usr/share/rpcd/acl.d/luci-app-flclash-webui.json $(1)/usr/share/rpcd/acl.d/luci-app-flclash-webui.json
	$(INSTALL_DIR) $(1)/usr/share/rpcd/ucode
	$(INSTALL_DATA) ./files/usr/share/rpcd/ucode/flclash.uc $(1)/usr/share/rpcd/ucode/flclash.uc

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
	$(INSTALL_DATA) ./webui/js/views.js $(1)/www/flclash/js/views.js
	$(INSTALL_DATA) ./webui/js/app.js $(1)/www/flclash/js/app.js
	$(INSTALL_DATA) ./webui/js/views/*.js $(1)/www/flclash/js/views/
	$(INSTALL_DATA) ./webui/fonts/material-symbols.woff2 $(1)/www/flclash/fonts/
	$(INSTALL_DATA) ./webui/img/icon.png $(1)/www/flclash/img/
endef

define Package/luci-app-flclash/postinst
#!/bin/sh
[ -n "$${IPKG_INSTROOT}" ] || {
	/etc/init.d/flclash-core enable
	/etc/init.d/flclash-api enable
	/etc/init.d/flclash-proxy enable
}
exit 0
endef

$(eval $(call BuildPackage,luci-app-flclash))
