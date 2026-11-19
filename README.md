<div align="center">

# FlClash for OpenWrt / ImmortalWrt

**单包 APK · mihomo + sing-box 双内核 · Rust/Tokio API 桥 · HTML5 WebUI**

<a href="https://github.com/LianXia233/luci-app-flclash/releases"><img alt="version" height="20" src="https://img.shields.io/badge/version-1.2.0-0f766e?style=flat-square"></a>
<a href="LICENSE"><img alt="license" height="20" src="https://img.shields.io/badge/license-GPL--3.0--only-0f766e?style=flat-square"></a>
<a href="#兼容性与限制"><img alt="arch" height="20" src="https://img.shields.io/badge/arch-aarch64__cortex--a53%20%7C%20x86__64-0891b2?style=flat-square"></a>
<a href="https://github.com/LianXia233/luci-app-flclash/actions/workflows/build-apk.yml"><img alt="CI" height="20" src="https://img.shields.io/github/actions/workflow/status/LianXia233/luci-app-flclash/build-apk.yml?style=flat-square&label=CI"></a>

[功能](#功能) · [快速开始](#快速开始) · [架构](#架构) · [配置](#配置项) · [API](#api-总览) · [构建](#构建) · [CI 与发布](#ci-与发布) · [兼容性](#兼容性与限制)

</div>

---

FlClash 的 OpenWrt 移植：把 **mihomo 内核**、**Rust IPC host**、**Rust/Tokio API 桥**、**LuCI 入口**与 **HTML5 WebUI** 装进**一个 apk**。

订阅只需导入一次。桥接将订阅归一为 mihomo `config.yaml`，切换到 sing-box 时再结构化转换为原生 JSON（节点、代理组、规则、DNS、TUN），混合端口与控制器端口保持不变。

## 功能

- **订阅导入**：Clash YAML、base64 节点列表，以及分享链接 `ss / ssr / vmess / vless / trojan / hysteria (v1) / hysteria2 / hy2 / tuic / anytls`。
- **配置管理**：订阅更新、编辑（名称 / URL / 自动更新间隔）、流量与到期展示、一键应用。
- **代理与连接**：代理组 / 节点测速与切换、连接列表与关闭、规则查看、日志采样。
- **DNS 进阶**：fake-ip / redir-host、DoH / DoT / bootstrap、DNS 查询与缓存刷新。
- **双内核**：内置 mihomo（FlClash native-IPC core，由 `flclash-core-bridge` 驱动）；sing-box 通过在线更新安装，以 `clash_api` 模式接入。WebUI 可查看版本、在线更新、一键切换。
- **优雅降级**：内核停止时读接口回退到 `config.yaml`，仍可查看状态与保存配置；依赖内核的写操作返回明确的 `503`，`apply` 会自动拉起内核。
- **LuCI 集成**：`服务 → FlClash` 单页，提供启停、内核更新、日志与一键进入 WebUI。

## 快速开始

### 安装

从 [Releases](https://github.com/LianXia233/luci-app-flclash/releases) 下载与设备架构、固件通道匹配的**单个 apk**：

| 固件 | 架构 | 文件后缀 |
| --- | --- | --- |
| ImmortalWrt 25.12.2 | ARM64-A53 | `..._aarch64_cortex-a53_immortalwrt-25.12.2.apk` |
| ImmortalWrt 25.12.2 | x86_64 | `..._x86_64_immortalwrt-25.12.2.apk` |
| ImmortalWrt snapshot | ARM64-A53 | `..._aarch64_cortex-a53_immortalwrt-snapshot.apk` |
| ImmortalWrt snapshot | x86_64 | `..._x86_64_immortalwrt-snapshot.apk` |

```sh
apk add --allow-untrusted ./luci-app-flclash_*.apk
```

> apk 由构建 SDK 签名，其公钥不在设备默认信任库中，因此首次安装需要 `--allow-untrusted`。

从旧版三包（`flclash-core` / `flclash-api` / `luci-app-flclash-webui`）升级时，先卸载旧包，避免文件归属冲突：

```sh
apk del flclash-core flclash-api luci-app-flclash-webui
apk add --allow-untrusted ./luci-app-flclash_*.apk
```

`postinst` 会自动启用 `flclash-core` 与 `flclash-api`。安装后：

1. 打开 LuCI：**服务 → FlClash**，确认服务运行；
2. 点击「一键进入 WebUI」，或直接访问 `http://<设备IP>:9098/flclash/`；
3. 在「配置」页导入订阅，应用后即可使用。

## 架构

```text
浏览器 ──► LuCI「服务 → FlClash」──► rpcd ubus 对象 flclash
                                      ├─► /etc/init.d/flclash-core   (mihomo / sing-box)
                                      └─► /etc/init.d/flclash-api    (Rust 桥接)

浏览器 ──► http://<IP>:9098/flclash/        静态 WebUI（与 API 同源）
        └─ /flclash/api/*                 Rust/Tokio 桥接
                                              ├─► 127.0.0.1:9097 controller
                                              ├─ mihomo   （native IPC + flclash-core-bridge）
                                              └─ sing-box （clash_api，由 config.yaml 转换）
```

| 组件 | 安装路径 | 说明 |
| --- | --- | --- |
| Go 内核 | `/usr/bin/mihomo` | FlClash native-IPC core，静态编译，`with_gvisor` |
| IPC host | `/usr/bin/flclash-core-bridge` | Rust；依次调用 `core.init` → `config.setup` → `listener.start` |
| API 桥 | `/usr/bin/flclash-api` | Rust / Tokio / axum；REST API + 静态 WebUI |
| procd 服务 | `/etc/init.d/flclash-core`、`flclash-api`、`flclash-proxy` | 内核、桥接、透明代理接管 |
| UCI 配置 | `/etc/config/flclash` | 唯一 conffile |
| 辅助脚本 | `/usr/libexec/flclash/*.sh` | 内核更新、透明代理规则、sing-box 转换 |
| LuCI | `/usr/share/luci/menu.d`、`/usr/share/rpcd/{acl.d,ucode}` | 菜单、ACL、ubus 对象 |
| WebUI | `/www/flclash/` | HTML / CSS / JS / 字体 / 图标 |

### 端口

| 端口 | 默认绑定 | 用途 |
| --- | --- | --- |
| `9097` | `127.0.0.1` | mihomo / sing-box 控制器（`external-controller` / `clash_api`） |
| `7892` | 本机（默认 `allow-lan: false`） | mixed HTTP/SOCKS 代理入口 |
| `7893` | `127.0.0.1` | 透明代理 TCP REDIRECT 入口（`redir-port`） |
| `7894` | `127.0.0.1` | 透明代理 UDP TPROXY 入口（`tproxy-port`） |
| `1053` | `0.0.0.0` | 内核 DNS 监听器（`dns.listen`），供 dnsmasq 与 LAN DNS 劫持使用 |
| `9098` | `0.0.0.0` | `/flclash/api` 与 `/flclash/` WebUI |

`9098` 默认对 LAN 开放。若 LAN 不完全可信，请在 UCI 中设置 `flclash.api.secret`，并为 WebUI 配置对应的请求头（见 [API 总览](#api-总览)）。

### 透明代理与出站模式

WebUI 的「出站模式」（`mode: rule / global / direct`）与「透明代理（局域网接管）」是两层独立配置，最终都由内核统一分流：

- **出站模式**：即 mihomo 的 `mode`，决定流量走规则 / 全局 / 直连；在「设置 → 网络」或首页模式条切换，运行时通过 controller `PATCH /configs` 生效。
- **透明代理（局域网接管）**：由 `POST /proxy/start` 调用 `proxy-rules.sh`，按检测到的防火墙后端（`fw4` → nftables）在 LAN 接口（`br-lan`）的 prerouting 链劫持流量：TCP `REDIRECT` 到 `7893`、UDP `TPROXY` 到 `7894`、DNS（TCP/UDP 53）`REDIRECT` 到 `1053`；同时写入 dnsmasq 片段将系统 DNS 指向 `127.0.0.1#1053`。私网地址段（RFC1918 等）被排除，路由器管理面（SSH / LuCI）不受影响。状态保存在 UCI `flclash.proxy.enabled`、`/tmp/flclash-proxy.state` 与 nft 规则三处，重启后由 `/etc/init.d/flclash-proxy` 恢复。
- **DNS（进阶）**：设置页写入 `config.yaml` 的 `dns` 块（fake-ip / redir-host、`nameserver`、`default-nameserver` 等）。后端在渲染该块时会固定写出 `dns.listen: 0.0.0.0:1053`，确保内核始终提供 DNS 监听；`proxy-rules.sh` 另有兜底自愈，启用接管时若发现该字段缺失会自动补回并重载内核。因此修改 DNS 设置不会破坏透明代理的 DNS 劫持。

典型旁路由拓扑下，`eth0` 为 WAN（上级路由），`br-lan` 为 LAN（客户端网关指向本机）；透明代理应挂在 `br-lan` 才能接管客户端流量。可用 `LAN_IF` 环境变量覆盖默认接口。

## 配置项

`/etc/config/flclash` 分为 `main`、`api`、`webui` 三段。首次启动时 `99-flclash-webui` 只补齐缺失键，不会覆盖已有配置或写入凭据。

<details>
<summary><b>完整 UCI 配置</b></summary>

| 段 | 选项 | 默认 | 说明 |
| --- | --- | --- | --- |
| `main` | `enabled` | `1` | 内核服务开关 |
| `main` | `config_file` | `/etc/flclash/config.yaml` | mihomo 配置路径 |
| `main` | `external_controller` | `127.0.0.1:9097` | 控制器监听地址 |
| `main` | `mixed_port` | `7892` | mixed 代理端口 |
| `main` | `mode` | `rule` | mihomo 运行模式 |
| `main` | `log_level` | `info` | 日志级别 |
| `main` | `core_type` | `mihomo` | `mihomo` / `sing-box` |
| `main` | `core_mode` | `ipc` | `ipc`（内置 FlClash core）/ `cli`（在线更新后的独立 mihomo） |
| `main` | `clash_api_port` | `9097` | 兼容 / 展示字段 |
| `main` | `tun_enabled` | `0` | TUN 开关（由 WebUI 配置块接管） |
| `main` | `tun_name` | 空 | TUN 设备名，空为自动 |
| `main` | `routing_mode` | 空 | 兼容 / 展示字段 |
| `main` | `ipv6` | `0` | IPv6 兼容字段 |
| `api` | `enabled` | `1` | 桥接服务开关 |
| `api` | `listen` | `0.0.0.0:9098` | API / WebUI 监听地址 |
| `api` | `ui_dir` | `/www/flclash` | 静态资源目录 |
| `api` | `data_dir` | `/etc/flclash` | 订阅与配置数据目录 |
| `api` | `secret` | 空 | 非空时启用请求鉴权 |
| `webui` | `enabled` | `1` | WebUI 开关 |
| `webui` | `webui_path` | `/flclash/` | WebUI 路径 |
| `webui` | `webui_port` | `9098` | WebUI 端口（与 `api.listen` 一致） |

</details>

## API 总览

所有接口以 `/flclash/api` 为前缀。若设置 `api.secret`，请求需携带：

```http
X-FlClash-Secret: <secret>
```

或：

```http
Authorization: Bearer <secret>
```

### 状态与内核

| 方法与路径 | 说明 |
| --- | --- |
| `GET /status` | 运行状态、出口 IP / 地区 / 质量、内存等 |
| `GET /version` | 内核与桥接版本 |
| `GET /memory` | 内存统计 |
| `POST /service/{action}` | `start` / `stop` / `restart` 内核服务 |
| `GET /system/proxy` | 系统透明代理状态 |
| `GET /proxy`、`POST /proxy/{action}` | 透明代理开关与状态 |

### 代理与连接

| 方法与路径 | 说明 |
| --- | --- |
| `GET /proxies` | 代理组与节点；内核停止时返回空集（200） |
| `POST /proxies/select` | 切换节点 / 组 |
| `POST /proxies/delay` | 单节点测速 |
| `POST /proxies/group-delay` | 整组测速 |
| `GET /connections` | 连接列表 |
| `DELETE /connections/{id}` | 关闭指定连接 |
| `GET /logs` | 日志采样（环形缓冲） |

### 配置、规则与 DNS

| 方法与路径 | 说明 |
| --- | --- |
| `GET /config`、`PUT /config`、`PATCH /config` | 标量与 `dns` 块；持久化到 `config.yaml` |
| `GET /rules` | 规则列表 |
| `GET /providers/proxies` | 代理提供者 |
| `GET /dns/resolve`、`GET /dns/queries`、`POST /dns/flush` | DNS 解析、查询历史、刷新缓存 |

### 订阅

| 方法与路径 | 说明 |
| --- | --- |
| `GET /profiles` | 订阅列表与流量 / 到期元数据 |
| `POST /profiles/import` | 下载 → 转换 → 落库 |
| `POST /profiles/{id}/update` | 更新订阅 |
| `POST /profiles/{id}/apply` | 转换 → 原子写 `config.yaml` → 重载或启动内核 |
| `PATCH /profiles/{id}`、`PUT /profiles/{id}`、`DELETE /profiles/{id}` | 编辑名称 / URL / 自动更新间隔，或删除 |

静态资源：

| 路径 | 说明 |
| --- | --- |
| `GET /`、`GET /flclash`、`GET /flclash/` | WebUI 入口 |
| `GET /flclash/*` | WebUI 文件 |

> 内核未运行时，读接口返回 `200` + `config.yaml` 文件回显；依赖内核的写操作返回 `503`，不回传底层 dial 错误。

## rpcd 对象

LuCI 侧通过 ubus 对象 `flclash` 调用（定义于 `files/usr/share/rpcd/ucode/flclash.uc`）：

| 方法 | 类型 | 说明 |
| --- | --- | --- |
| `system_proxy` | 读 | 系统代理状态摘要 |
| `service_state` | 读 | `{ running, enabled }` |
| `service_toggle` | 写 | `start` / `stop` / `restart` |
| `core_log` | 读 | `logread` 中的内核日志 |
| `core_update` | 写 | 触发 `core-update.sh` |
| `core_update_status` | 读 | 更新状态、日志、版本 |
| `core_type` | 读 | 当前内核与已安装二进制 |
| `core_switch` | 写 | 持久化 `mihomo` / `sing-box` 选择 |

## 构建

### 前置

| 工具 | 用途 |
| --- | --- |
| Rust stable + `rust-std` musl target | 编译 `flclash-api` 与 `flclash-core-bridge` |
| Go 1.27 | 编译 FlClash IPC 内核 |
| OpenWrt SDK | 仅组装 / 签名 apk，**不编译 Rust** |
| `make` / `gcc` / `perl` / `rsync` / `zstd` | SDK 打包所需主机工具 |

### Rust 桥接（官方 SDK）

`flclash-api/.cargo/config.toml` 已固定 `rust-lld` 与 `link-self-contained` 参数：

```bash
rustup target add aarch64-unknown-linux-musl x86_64-unknown-linux-musl

cd flclash-api
cargo test --locked
cargo build --release --locked --target aarch64-unknown-linux-musl --bins
cargo build --release --locked --target x86_64-unknown-linux-musl --bins
```

产物为 `target/<target>/release/{flclash-api,flclash-core-bridge}`，静态 ELF，约 1.3 MiB（ARM64）/ 1.5 MiB（x86_64）。

### Go 内核

```bash
./scripts/build-core.sh luci-app-flclash/files/usr/bin
./scripts/verify-core.sh luci-app-flclash/files/usr/bin
```

脚本默认构建 `arm64 / armv7 / x86_64 / mipsle` 四种内核；发布 CI 只取 `arm64` 与 `amd64`，分别使用 `GOARM64=v8.0` 与 `GOAMD64=v1`。

### 组装 apk

把三个预编译二进制放入 `luci-app-flclash/files/usr/bin/` 后，用 SDK 组装：

```bash
cp -a luci-app-flclash "$SDK/package/luci-app-flclash"
cd "$SDK"
make defconfig
make -j"$(nproc)" package/luci-app-flclash/compile V=s
```

单包 Makefile 的 `Build/Compile` 为空，SDK 只执行安装与打包；Rust 与 Go 的编译全部发生在 SDK 之外。

## CI 与发布

[`.github/workflows/build-apk.yml`](.github/workflows/build-apk.yml) 是「二进制 → 打包 → 发布」三段流水线：

```text
binaries (2) ──► package (4) ──► release (1)
```

| 阶段 | 运行内容 | 关键校验 |
| --- | --- | --- |
| `binaries` | rustup 官方 SDK + Go 官方工具链；两架构并行 | `cargo test --locked`；ELF / 静态 / 目标架构 |
| `package` | ImmortalWrt SDK 只组装与签名；四组合并行 | 架构闸门、无 `INTERP`、**恰好 1 个 apk** |
| `release` | 汇总产物、生成 `sha256sums.txt` | **恰好 4 个 apk**，再发布 |

### 构建矩阵

| 通道 | SDK | 架构 | 目标 |
| --- | --- | --- | --- |
| `immortalwrt-25.12.2` | `immortalwrt-sdk-25.12.2-*_gcc-*_musl` | `aarch64_cortex-a53` | `mediatek/filogic` |
| `immortalwrt-25.12.2` | `immortalwrt-sdk-25.12.2-x86-64_*` | `x86_64` | `x86/64` |
| `immortalwrt-snapshot` | `immortalwrt-sdk-*_gcc-*_musl` | `aarch64_cortex-a53` | `mediatek/filogic` |
| `immortalwrt-snapshot` | `immortalwrt-sdk-x86-64_*` | `x86_64` | `x86/64` |

### 触发与产物

- `push` 到 `main`：改动 `luci-app-flclash/**`、`flclash-api/**`、`scripts/**`、workflow 或 `README.md` 时触发构建（不发布）。
- `push` 标签 `v*`：构建并发布。
- 手动：`gh workflow run build-apk.yml -f publish=true`。

产物命名：

```text
luci-app-flclash_<ver>-r<rel>_<arch>_immortalwrt-25.12.2.apk
luci-app-flclash_<ver>-r<rel>_<arch>_immortalwrt-snapshot.apk
```

每个 Release 固定 5 个资产：4 个单包 apk + `sha256sums.txt`。

当前 `v1.2.0` 实测体积：

| 架构 / 通道 | 大小 |
| --- | --- |
| ARM64-A53 / 25.12.2 | 28,362,844 B（27.05 MiB） |
| ARM64-A53 / snapshot | 28,362,805 B（27.05 MiB） |
| x86_64 / 25.12.2 | 30,772,515 B（29.35 MiB） |
| x86_64 / snapshot | 30,772,703 B（29.35 MiB） |

## 兼容性与限制

- **固件**：ImmortalWrt **25.12.2** 与 **snapshot**，两者各自产出独立单包；同为 apk 包格式。
- **架构**：`aarch64_cortex-a53`（ARM64-A53）与 `x86_64`（通用 `GOAMD64=v1`）。其他架构需自行用脚本编译，发布矩阵暂不覆盖。
- **静态二进制**：`mihomo`、`flclash-api`、`flclash-core-bridge` 均为静态 ELF，无 glibc / musl 运行时依赖，无 `INTERP`。
- **内核依赖**：透明代理需要设备内核具备 `TUN` 与 `nftables`（ImmortalWrt 默认镜像已带 `firewall4` / `nftables`）。这些 `kmod-*` 未在 25.12 的 apk 源发布，因此**不写入硬依赖**，避免 `apk add` 解依赖失败。
- **sing-box 不随包内置**：单包只带 mihomo。sing-box 通过 LuCI / WebUI 的「更新内核」在线安装到 `/usr/bin/sing-box`，再由 `core_type=sing-box` 启用。
- **在线更新依赖网络**：内核更新从 GitHub Releases 下载，设备需能访问 `api.github.com` 与 release 资源；受限网络下可手动放置二进制。
- **安装签名**：构建 SDK 的 apk 签名不在设备默认信任库，需 `--allow-untrusted`。
- **升级**：旧版三包与单包写入相同路径，升级前先 `apk del` 旧三包。
- **安全**：`9098` 默认监听 `0.0.0.0`。在不可信 LAN 中请设置 `flclash.api.secret`，或改为仅监听回环并自行做反代。

## 验证

仓库内可复现的检查：

| 检查 | 方式 | 结果 |
| --- | --- | --- |
| Rust 单元测试 | `cd flclash-api && cargo test --locked` | 15 passed / 0 failed |
| 二进制架构与静态性 | CI `binaries` / `package` 架构闸门 | ELF、静态、目标架构、无 `INTERP` |
| 单包约束 | `package` job 断言 | 每个组合**恰好 1 个** apk |
| 发布一致性 | `release` job + `sha256sum -c` | 4 个 apk 全部匹配 |
| 包内容 | 解包核对 | 38 个文件（35 个有效文件 + 3 个 apk 元数据） |

设备端透明代理与 DNS 的复核方法：

```sh
# 1. 状态三处一致：UCI 开关 / 状态文件 / nft 规则
uci get flclash.proxy.enabled          # 应为 1
cat /tmp/flclash-proxy.state           # 应为 on
/usr/libexec/flclash/proxy-rules.sh status  # 应为 ACTIVE (nftables)

# 2. 劫持规则挂在正确的 LAN 接口（旁路由通常是 br-lan）
nft list table inet flclash | grep iifname
#   应见 br-lan 的 53 -> 1053、TCP -> 7893、UDP -> TPROXY 7894

# 3. 内核各监听端口就绪（含 DNS 1053）
netstat -lntup | grep -E ':1053|:7893|:7894|:9097'

# 4. DNS 经劫持返回 fake-ip（198.18.0.0/15）
nslookup www.example.com 127.0.0.1     # 应返回 198.18.x.x

# 5. LAN 客户端默认网关指向本机，访问外网应出现在连接列表
wget -q -O - http://<设备IP>:9098/flclash/api/connections
```

若第 4 步返回真实 IP 或超时，先检查 `config.yaml` 的 `dns.listen` 是否为 `0.0.0.0:1053`，再确认 dnsmasq 片段 `/tmp/dnsmasq.d/flclash-dns.conf` 是否指向 `127.0.0.1#1053`。管理面（SSH / LuCI）走私网地址，不在劫持范围内，应始终可用。

设备端的 TUN 接管、内核切换与分流效果取决于具体固件与网络环境，建议在目标机型上按[快速开始](#快速开始)复核。

## 目录结构

```text
.
├── .github/workflows/build-apk.yml   # 二进制 → 打包 → 发布
├── flclash-api/                      # Rust crate
│   ├── src/main.rs                   # axum 路由与鉴权
│   ├── src/{config,convert,profiles,singbox,loops}.rs
│   └── src/bin/flclash-core-bridge.rs
├── luci-app-flclash/
│   ├── Makefile                      # 唯一 BuildPackage
│   ├── files/                        # 配置、init、脚本、LuCI、rpcd
│   └── webui/                        # HTML5 WebUI（css / js / views / fonts / img）
├── scripts/
│   ├── build-core.sh                 # Go 内核交叉编译
│   └── verify-core.sh                # 静态性与架构校验
└── README.md
```

WebUI 按视图拆分：`profiles`、`proxies`、`requests`、`rules`、`dns`、`logs`、`settings`、`tools`，共 8 个视图；首页仪表盘展示出口 IP、地区与质量。

## 许可证与致谢

- 本项目：**GPL-3.0-only**，见 [LICENSE](LICENSE)。
- 上游：[FlClash](https://github.com/chen08209/FlClash)（IPC core）、[mihomo](https://github.com/MetaCubeX/mihomo)、[sing-box](https://github.com/SagerNet/sing-box)。

> 本项目不向项目作者收集订阅或设备凭据。为展示出口 IP 归属与质量，运行时会向 `ip-api.com`、`api.ip.sb` 发起查询（会暴露当前出口 IP）；其余数据仅保存在设备本地。
