include $(TOPDIR)/rules.mk

PKG_NAME:=flclash-api
PKG_VERSION:=1.0.0
PKG_RELEASE:=1

PKG_MAINTAINER:=FlClash WebUI
PKG_LICENSE:=GPL-3.0-only

include $(INCLUDE_DIR)/package.mk

define Package/flclash-api
  SECTION:=net
  CATEGORY:=Network
  SUBMENU:=Virtual Private Network (VPN)
  TITLE:=FlClash WebUI API bridge
  DEPENDS:=+flclash-core
  PKGARCH:=$(if $(ARCH_PACKAGES),$(ARCH_PACKAGES),$(ARCH))
endef

define Package/flclash-api/description
  Static Go bridge (stdlib only) exposing the FlClash WebUI contract on top of
  mihomo's loopback controller, and serving /flclash/ from the same origin.
endef

define Build/Prepare
	mkdir -p $(PKG_BUILD_DIR)
endef

define Build/Compile
endef

define Package/flclash-api/install
	$(INSTALL_DIR) $(1)/usr/bin
	$(INSTALL_BIN) ./files/usr/bin/flclash-api $(1)/usr/bin/flclash-api
	$(INSTALL_DIR) $(1)/etc/init.d
	$(INSTALL_BIN) ./files/etc/init.d/flclash-api $(1)/etc/init.d/flclash-api
endef

define Package/flclash-api/postinst
#!/bin/sh
[ -n "$${IPKG_INSTROOT}" ] || /etc/init.d/flclash-api enable
exit 0
endef

$(eval $(call BuildPackage,flclash-api))
