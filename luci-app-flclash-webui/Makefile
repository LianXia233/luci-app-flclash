include $(TOPDIR)/rules.mk

PKG_NAME:=luci-app-flclash-webui
PKG_VERSION:=1.1.0
PKG_RELEASE:=4

PKG_MAINTAINER:=FlClash WebUI
PKG_LICENSE:=GPL-3.0-only

include $(INCLUDE_DIR)/package.mk

define Package/luci-app-flclash-webui
  SECTION:=luci
  CATEGORY:=LuCI
  SUBMENU:=3. Applications
  TITLE:=FlClash WebUI launcher
  DEPENDS:=+flclash-core +flclash-api +luci-base +rpcd +rpcd-mod-ucode
  PKGARCH:=all
endef

define Package/luci-app-flclash-webui/description
  HTML5 WebUI for FlClash. LuCI only starts the Go core and redirects to
  /flclash/; every device operation happens in the WebUI itself.
endef

define Build/Compile
endef

define Package/luci-app-flclash-webui/install
	# LuCI menu + launcher view + ACL
	$(INSTALL_DIR) $(1)/usr/share/luci/menu.d
	$(INSTALL_DATA) ./files/usr/share/luci/menu.d/luci-app-flclash-webui.json $(1)/usr/share/luci/menu.d/

	$(INSTALL_DIR) $(1)/www/luci-static/resources/view/flclash
	$(INSTALL_DATA) ./files/www/luci-static/resources/view/flclash/launch.js $(1)/www/luci-static/resources/view/flclash/launch.js

	$(INSTALL_DIR) $(1)/usr/share/rpcd/acl.d
	$(INSTALL_DATA) ./files/usr/share/rpcd/acl.d/luci-app-flclash-webui.json $(1)/usr/share/rpcd/acl.d/

	$(INSTALL_DIR) $(1)/usr/share/rpcd/ucode
	$(INSTALL_DATA) ./files/usr/share/rpcd/ucode/flclash.uc $(1)/usr/share/rpcd/ucode/flclash.uc

	$(INSTALL_DIR) $(1)/usr/libexec/flclash
	$(INSTALL_BIN) ./files/usr/libexec/flclash/core-update.sh $(1)/usr/libexec/flclash/core-update.sh

	$(INSTALL_DIR) $(1)/etc/uci-defaults
	$(INSTALL_BIN) ./files/etc/uci-defaults/99-flclash-webui $(1)/etc/uci-defaults/

	# Static WebUI assets
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

$(eval $(call BuildPackage,luci-app-flclash-webui))
