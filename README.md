# luci-app-flclash

FlClash WebUI + mihomo 静态内核的 OpenWrt 适配包，面向 OpenWrt 主线（master）与 SNAPSHOT 快照版，目标架构 `aarch64_cortex-a53`。

## 设计边界

- **LuCI 只做两件事**：启动 flclash 服务、跳转到 WebUI（`/flclash/`）。
- **所有设备操作由 WebUI 承接**：配置、代理、连接、DNS、日志、设置均在 HTML5 WebUI 内完成。
- **内核保持 Go 实现**：复用 `chen08209/FlClash` 的 `core/Clash.Meta`（mihomo fork），不引入 Flutter/Rust。
- **系统代理信息只读**：`FlClashAPI.systemProxy()` 经 rpcd/ubus 只读接口获取，不含任何凭据。

## 目录结构

```
.
├── .github/workflows/build-apk.yml     # 云编译 workflow
├── flclash-core/                        # 静态内核包（编译时注入二进制）
│   ├── Makefile
│   └── files/etc/{config,init.d}
├── luci-app-flclash-webui/              # LuCI 启动入口 + WebUI
│   ├── Makefile
│   ├── files/                           # menu.d / ACL / ucode / uci-defaults / launch.js
│   └── webui/                           # HTML5 + CSS + JS 静态资源
└── scripts/                             # 本地编译与兼容性验证脚本
```

## 云编译产出 apk

仓库不提交预编译二进制。推送后由 GitHub Actions 云端完成：

1. 拉取 `chen08209/FlClash` 源码与 `core/Clash.Meta` 子模块；
2. Go 1.27.3 交叉编译 `linux/arm64` 静态内核（`CGO_ENABLED=0`、`with_gvisor`、`-w -s`）；
3. 下载 OpenWrt SNAPSHOT SDK（`armsr/armv8`，musl），将本仓库两个包置入 `package/`；
4. `make package/.../compile` 产出 `.apk`；
5. 上传为 Actions artifact，并发布到 Release 标签 `apk-aarch64-cortex-a53`。

触发方式：向 `main` 推送、打 `v*` 标签，或在 Actions 页手动 `workflow_dispatch`。

## 本地构建

```bash
./scripts/build-core.sh
./scripts/verify-core.sh ./flclash-core/files/usr/bin
```

## 安装后在设备上

- 服务：`/etc/init.d/flclash-core`（procd 托管，默认开机自启）
- 配置：`/etc/config/flclash`
- 内核：`/usr/bin/mihomo`
- WebUI：LuCI 菜单「服务 → FlClash WebUI」启动并跳转，或直接访问 `/flclash/`
- 依赖：`kmod-tun`、`kmod-nft-core`、`kmod-nft-nat`、`kmod-nft-fib`

## 兼容性

- OpenWrt master / SNAPSHOT：默认 apk（`PACKAGE_SUFFIX=apk`），本 workflow 产 `.apk`。
- 静态二进制无 glibc/musl 运行时依赖，内核 6.x 满足 gVisor TUN 要求。
- `aarch64_cortex-a53` 已验证；其他架构可用 `scripts/build-core.sh` 交叉编译后自行打包。
