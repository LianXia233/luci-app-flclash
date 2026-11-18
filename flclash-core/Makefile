include $(TOPDIR)/rules.mk

PKG_NAME:=flclash-core
PKG_VERSION:=1.19.33
PKG_RELEASE:=1

PKG_MAINTAINER:=FlClash WebUI
PKG_LICENSE:=GPL-3.0-only

# Prebuilt static Go core. CGO_ENABLED=0 + with_gvisor, built with Go 1.27.3.
# Binary assets are architecture-specific; select the matching prebuilt file.
ifeq ($(ARCH),aarch64)
  CORE_BIN:=mihomo-arm64
  PKGARCH:=aarch64_cortex-a53
else ifeq ($(ARCH),arm)
  CORE_BIN:=mihomo-armv7
  PKGARCH:=arm_cortex-a7_neon-vfpv4
else ifeq ($(ARCH),x86_64)
  CORE_BIN:=mihomo-amd64
  PKGARCH:=x86_64
else ifeq ($(ARCH),mipsel)
  CORE_BIN:=mihomo-mipsle
  PKGARCH:=mipsel_24kc
else
  CORE_BIN:=mihomo-arm64
  PKGARCH:=all
endif

include $(INCLUDE_DIR)/package.mk

define Package/flclash-core
  SECTION:=net
  CATEGORY:=Network
  SUBMENU:=Virtual Private Network (VPN)
  TITLE:=FlClash mihomo static core
  DEPENDS:=+kmod-tun +kmod-nft-core +kmod-nft-nat +kmod-nft-fib
  PKGARCH:=$(PKGARCH)
endef

define Package/flclash-core/description
  Static mihomo (FlClash Clash.Meta fork) core for OpenWrt master/SNAPSHOT.
  Built with CGO_ENABLED=0, no glibc/musl runtime dependency.
endef

define Package/flclash-core/conffiles
/etc/config/flclash
endef

define Build/Prepare
	mkdir -p $(PKG_BUILD_DIR)
endef

define Build/Compile
endef

define Package/flclash-core/install
	$(INSTALL_DIR) $(1)/usr/bin
	$(INSTALL_BIN) ./files/usr/bin/$(CORE_BIN) $(1)/usr/bin/mihomo
	$(INSTALL_DIR) $(1)/etc/config
	$(INSTALL_DATA) ./files/etc/config/flclash $(1)/etc/config/flclash
	$(INSTALL_DIR) $(1)/etc/init.d
	$(INSTALL_BIN) ./files/etc/init.d/flclash-core $(1)/etc/init.d/flclash-core
endef

define Package/flclash-core/postinst
#!/bin/sh
[ -n "$${IPKG_INSTROOT}" ] || {
	/etc/init.d/flclash-core enable
}
exit 0
endef

$(eval $(call BuildPackage,flclash-core))
