<div align="center">

# FlClash for OpenWrt / ImmortalWrt

**单包 APK · mihomo + sing-box 双内核 · Rust/Tokio API 桥 · HTML5 WebUI**

<a href="https://github.com/LianXia233/luci-app-flclash/releases"><img alt="version" height="20" src="https://img.shields.io/badge/version-1.3.0-0f766e?style=flat-square"></a>
<a href="LICENSE"><img alt="license" height="20" src="https://img.shields.io/badge/license-GPL--3.0--only-0f766e?style=flat-square"></a>
<a href="#兼容性与限制"><img alt="arch" height="20" src="https://img.shields.io/badge/arch-aarch64__cortex--a53%20%7C%20x86__64-0891b2?style=flat-square"></a>
<a href="https://github.com/LianXia233/luci-app-flclash/actions/workflows/build-apk.yml"><img alt="CI" height="20" src="https://img.shields.io/github/actions/workflow/status/LianXia233/luci-app-flclash/build-apk.yml?style=flat-square&label=CI"></a>

[功能](#功能) · [快速开始](#快速开始) · [架构](#架构) · [透明代理](#透明代理状态机) · [状态卡](#状态卡与-ip-资料) · [配置](#配置项) · [API](#api-总览) · [构建](#构建) · [CI 与发布](#ci-与发布) · [兼容性](#兼容性与限制)

</div>

---

FlClash 的 OpenWrt 移植：把 **mihomo 内核**、**Rust IPC host**、**Rust/Tokio API 桥**、**LuCI 入口**与 **HTML5 WebUI** 装进**一个 apk**。

订阅只需导入一次。桥接将订阅归一为 mihomo `config.yaml`，切换到 sing-box 时再结构化转换为原生 JSON（节点、代理组、规则、DNS、TUN），混合端口与控制器端口保持不变。

## 功能

- **订阅导入**：Clash YAML、base64 节点列表，以及分享链接 `ss / ssr / vmess / vless / trojan / hysteria (v1) / hysteria2 / hy2 / tuic / anytls`。
- **配置管理**：订阅更新、编辑（名称 / URL / 自动更新间隔）、流量与到期展示、一键应用。
- **代理与连接**：代理组 / 节点测速与切换、连接列表与关闭、规则查看、日志采样。
- **DNS 进阶**：fake-ip / redir-host / mixed、DoH / DoT / bootstrap、DNS 查询与缓存刷新。
- **透明代理状态机**：REDIRECT / TPROXY / TUN 三种接管路径由 `proxy-rules.sh sync` 统一收敛（见[透明代理状态机](#透明代理状态机)）；硬件流量卸载自动关闭；核心异常退出自动清理残留规则。
- **双栈 IP 状态卡**：出口 IPv4 / IPv6 同屏显示，多源查询（Cloudflare 优先）+ 中文归属地 + 启停状态感知（见[状态卡与 IP 资料](#状态卡与-ip-资料)）。
- **双内核**：内置 mihomo（FlClash native-IPC core，由 `flclash-core-bridge` 驱动）；sing-box 通过在线更新安装，以 `clash_api` 模式接入。WebUI 可查看版本、在线更新、一键切换。
- **优雅降级**：内核停止时读接口回退到 `config.yaml`，仍可查看状态与保存配置；依赖内核的写操作返回明确的 `503`，`apply` 会自动拉起内核。
- **LuCI 集成**：`服务 → FlClash` 单页。**开关只控制 WebUI 后端（flclash-api）**，代理内核由 WebUI 内的开关手动启停；「一键进入 WebUI」会在后端未运行时自动拉起。

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

安装后的服务自启策略（`99-flclash-webui`）：

| 服务 | 开机自启 | 说明 |
| --- | --- | --- |
| `flclash-api` | ✅ | WebUI + Rust 后端（`9098`） |
| `flclash-proxy` | ✅ | 接管状态收敛器（按 UCI 期望值恢复接管） |
| `flclash-core` | ❌ | 代理内核**不随开机启动**，由用户在 WebUI 手动开启 |

安装后：

1. 打开 LuCI：**服务 → FlClash**，「一键进入 WebUI」（后端未运行时会自动启动）；
2. 在 WebUI 仪表盘点击**启动内核**（蓝色圆形按钮或侧栏核心状态图标）；
3. 在「设置」开启**局域网接管**（或在「配置」页导入订阅后应用），即可上网。

## 架构

```text
浏览器 ──► LuCI「服务 → FlClash」──► rpcd ubus 对象 flclash
                                      └─► /etc/init.d/flclash-api    (WebUI + Rust 后端)

浏览器 ──► http://<IP>:9098/flclash/        静态 WebUI（与 API 同源）
        └─ /flclash/api/*                 Rust/Tokio 桥接（reqwest + rustls 原生出站）
                                              ├─► 127.0.0.1:9097 controller
                                              ├─ mihomo   （native IPC + flclash-core-bridge）
                                              └─ sing-box （clash_api，由 config.yaml 转换）

内核启停 ──► WebUI 核心开关 ──► /etc/init.d/flclash-core ──► 启停后 proxy-rules.sh sync 收敛接管
```

| 组件 | 安装路径 | 说明 |
| --- | --- | --- |
| Go 内核 | `/usr/bin/mihomo` | FlClash native-IPC core，静态编译，`with_gvisor` |
| IPC host | `/usr/bin/flclash-core-bridge` | Rust；依次调用 `core.init` → `config.setup` → `listener.start` |
| API 桥 | `/usr/bin/flclash-api` | Rust / Tokio / axum；REST API + 静态 WebUI + 原生 HTTPS 出站 |
| procd 服务 | `/etc/init.d/flclash-core`、`flclash-api`、`flclash-proxy` | 内核（手动）、桥接、接管收敛 |
| UCI 配置 | `/etc/config/flclash` | 唯一 conffile |
| 辅助脚本 | `/usr/libexec/flclash/*.sh` | 内核更新、透明代理状态机、sing-box 转换 |
| LuCI | `/usr/share/luci/menu.d`、`/usr/share/rpcd/{acl.d,ucode}` | 菜单、ACL、ubus 对象 |
| WebUI | `/www/flclash/` | HTML / CSS / JS / 字体 / 图标 |

### 端口

| 端口 | 绑定 | 用途 |
| --- | --- | --- |
| `9097` | `127.0.0.1` | mihomo / sing-box 控制器（`external-controller` / `clash_api`） |
| `7892` | `*`（接管时 `allow-lan` 被强制开启） | mixed HTTP/SOCKS 代理入口 |
| `7893` | `*` | 透明代理 TCP REDIRECT 入口（`redir-port`） |
| `7894` | `*` | 透明代理 UDP TPROXY 入口（`tproxy-port`） |
| `1053` | `*` | 内核 DNS 监听器（`dns.listen`），供 dnsmasq 与 LAN DNS 劫持使用 |
| `9098` | `0.0.0.0` | `/flclash/api` 与 `/flclash/` WebUI |

接管生效时 `allow-lan` 被强制为 `true`（REDIRECT 的目的地址是接口地址而非回环，绑死 `127.0.0.1` 会让 LAN 劫持连接全部被拒——这是「本机能代理、下挂设备不通」的根因）。作为补偿，`fw4 input` 会插入 **FlClash WAN Proxy Guard**：非 LAN 接口访问 `7893/7894` 直接 reject，避免在 WAN 侧变成开放代理。

`9098` 默认对 LAN 开放。若 LAN 不完全可信，请在 UCI 中设置 `flclash.api.secret`，并为 WebUI 配置对应的请求头（见 [API 总览](#api-总览)）。

## 透明代理状态机

所有接管状态由 `proxy-rules.sh sync` **统一收敛**，任何入口（WebUI 开关、内核启停、开机恢复、60 秒自愈循环）都汇入同一状态机：

```text
            ┌─ 内核未运行 ──► 全部拆除（REDIRECT 表 / DNS 劫持 / TUN 残留工件）
sync() ─────┼─ config.tun.enable=true 且 TUN 在线 ──► TUN 模式（内核接管，撤下 REDIRECT 表）
            ├─ 其余 + UCI wish=1 ──► REDIRECT 模式（LAN 劫持表 + dnsmasq→1053 + 本机 OUTPUT）
            └─ 其余 ──► 全部拆除
```

| 要点 | 说明 |
| --- | --- |
| **管理面豁免** | prerouting 只挂 `br-lan` 且排除 RFC1918 目的地址；本机 OUTPUT 链带 `ct direction reply accept` 与 `ct original ip daddr private accept`——即使客户端是公网地址，SSH / LuCI 的回包也不会被劫持 |
| **TUN 模式** | WebUI「虚拟网卡」开关写入 `tun:` 块并**完整重启内核**（IPC 内核热重载不会创建 TUN 设备）；接管期间撤下 REDIRECT 表与 DNS 手工接管，由内核 `Meta` 设备 + 策略路由接管 LAN，本机流量仍由 OUTPUT 链送入内核；`route-exclude-address` 固定排除全部管理/内网网段 |
| **本机接管** | nft `output` 链把本机非私网 TCP REDIRECT 到 `7893`，路由器自身请求同样经内核分流；环路防护依赖 `routing-mark: 333`（内核 socket 携带 `0x14d` mark 首条规则放行） |
| **残留清理** | 内核被 SIGKILL / 异常退出会在 ip rules 9000-9010、table 2022、nft `inet mihomo` 留下指向死端口的残留规则，把 LAN 打进黑洞——`start_service` 与 `sync` 都会清除 |
| **硬件卸载** | `flow_offloading` / `flow_offloading_hw` 被强制关闭，`bridger` 被停用并禁自启：硬件快路径的流绕过 netfilter，会让 REDIRECT 对 LAN TCP 失效（UDP 走 mangle 链幸存，更具迷惑性）；仅在此刻一次性 `conntrack -F` |
| **DNS 过滤** | 客户端 53 端口被直接劫持到内核 `1053`，不经过 dnsmasq，所以 dnsmasq 层 `filter_aaaa` 是无效摆设——改为在源头设 `dns.ipv6: false`（`flclash.proxy.filter_aaaa=1` 默认），双栈客户端回退 v4 进入接管路径 |
| **缩进自适应** | `prepare_config` 对 `config.yaml` 的所有插入（`listen` / `routing-mark` / `dns.ipv6` / TUN `route-exclude-address`）按 dns 块自身的缩进对齐——混合缩进会让 YAML 解析失败、核心无限 respawn |
| **出站模式** | `mode: rule / global / direct` 经 controller 运行时生效并持久化；sing-box 无运行时切换，由转换器按模式生成静态路由 |

状态落在三处：UCI `flclash.proxy.enabled`（用户期望，重启保留）、`/tmp/flclash-proxy.state`（`on` / `tun` / `off`，收敛结果）、nft 规则本身。`flclash-core` 每次启动前清理残留，启动后 5 秒与 API 每 60 秒各收敛一次。

## 状态卡与 IP 资料

仪表盘状态卡的出口 IP 由 `ipinfo.rs` 独立模块维护：

- **多源链**（按用户偏好排序）：Cloudflare `cdn-cgi/trace` 优先（恒可达、反映真实出口）→ `ip.api.skk.moe/cf-geoip` → ipfinal.com 卡片（中文省市）→ `api.ip.sb` → `ipwho.is` → `ip-api.com`（zh-CN，最后兜底）。
- **双栈显示**：节点有 IPv6 出口时，IPv4 与 IPv6 同屏显示（v6 通过 CF anycast v6 固定地址采样，不依赖 DNS，不受 AAAA 过滤影响）；节点无 v6 则隐藏该行。
- **启停感知**：内核启停会切换出口路径（代理出口 ↔ 直连 WAN），采样在接管状态文件回位后立即重跑——「已停止」状态下显示的是真实直连出口，且保持更新。
- **节点抖动重试**：代理组 url-test 偶发选到故障节点会导致整链超时，最多 3 轮整链重试（间隔 3 秒）。
- **汉化与占位符过滤**：显示无中文时自动走 zh-CN 源补中文归属；`Private Customer`、裸 RIR 名等占位符永不显示，无主 IP 回退「中文地区 · ASxxxx」格式。
- **显示对齐**：IP 资料与运行时长统一字号与基线，长名称自动换行不省略（桌面与手机一致）。
- HTTP(S) 全部为 Rust 原生出站（reqwest + rustls/webpki-roots），客户端强制 IPv4 绑定，确保采样跟随 v4 接管路径。

## 配置项

`/etc/config/flclash` 分为 `main`、`proxy`、`api`、`webui` 四段。首次启动时 `99-flclash-webui` 只补齐缺失键，不会覆盖已有配置或写入凭据。

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
| `main` | `tun_enabled` | `0` | TUN 开关（由 WebUI 配置块接管） |
| `api` | `enabled` | `1` | 桥接服务开关 |
| `api` | `listen` | `0.0.0.0:9098` | API / WebUI 监听地址 |
| `api` | `ui_dir` | `/www/flclash` | 静态资源目录 |
| `api` | `data_dir` | `/etc/flclash` | 订阅与配置数据目录 |
| `api` | `secret` | 空 | 非空时启用请求鉴权 |
| `proxy` | `enabled` | `1` | 局域网接管期望值（重启保留） |
| `proxy` | `backend` | 自动 | `nftables` / `iptables`（自动检测） |
| `proxy` | `lan_if` | 自动 | 接管接口（默认 ubus 检测 LAN 设备） |
| `proxy` | `filter_aaaa` | `1` | 接管时过滤 AAAA（源头上设 `dns.ipv6: false`） |
| `proxy` | `saved_*` / `dns_filter_managed` | 空 | dns_off 恢复 dnsmasq 原状用 |
| `webui` | `enabled` | `1` | WebUI 开关 |
| `webui` | `webui_path` | `/flclash/` | WebUI 路径 |
| `webui` | `webui_port` | `9098` | WebUI 端口（与 `api.listen` 一致） |

</details>

## API 总览

所有接口以 `/flclash/api` 为前缀。若设置 `api.secret`，请求需携带 `X-FlClash-Secret: <secret>` 或 `Authorization: Bearer <secret>`。

### 状态与内核

| 方法与路径 | 说明 |
| --- | --- |
| `GET /status` | 运行状态、出口 IP（`outboundIp` / `outboundIp6`）/ 归属 / 质量、`takeoverState`、内存等 |
| `GET /version` | 内核与桥接版本 |
| `GET /memory` | 内存统计 |
| `POST /service/{action}` | `start` / `stop` / `restart` 内核服务（启停后自动收敛接管） |
| `GET /system/proxy` | 系统透明代理状态摘要 |
| `GET /proxy`、`POST /proxy/{action}` | 透明代理开关与状态（`takeover` / `tunActive` / `coreRunning` / `proxyWish`） |

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
| `GET /config`、`PUT /config`、`PATCH /config` | 标量（`mode` / `tun` / `proxyLan`…）与 `dns` 块；持久化到 `config.yaml`；`tun` 变更触发内核完整重启并收敛接管 |
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

静态资源：`GET /flclash/`（WebUI 入口）与 `GET /flclash/*`。

> 内核未运行时，读接口返回 `200` + `config.yaml` 文件回显；依赖内核的写操作返回 `503`，不回传底层 dial 错误。

## rpcd 对象

LuCI 侧通过 ubus 对象 `flclash` 调用（定义于 `files/usr/share/rpcd/ucode/flclash.uc`）：

| 方法 | 类型 | 说明 |
| --- | --- | --- |
| `system_proxy` | 读 | 系统代理状态摘要 |
| `service_state` | 读 | `{ running, enabled }` —— **flclash-api（WebUI 后端）的状态** |
| `service_toggle` | 写 | `start` / `stop` / `restart` —— 只作用于 flclash-api |
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
| **aarch64 musl C 交叉编译器** | `ring`（reqwest/rustls 引入）需要 C 工具链编译目标平台代码 |
| Go 1.27 | 编译 FlClash IPC 内核 |
| OpenWrt SDK | 仅组装 / 签名 apk，**不编译 Rust** |

### Rust 桥接（WSL 交叉编译推荐）

`ring` 的构建脚本需要 C 交叉编译器，Windows 的 rust-lld 环境无法提供，推荐在 WSL 内编译（仓库根的 `build-wsl.sh` 封装了拷贝源码 + 交叉编译 + 回传产物）：

```bash
# WSL 内一次性环境
curl https://sh.rustup.rs -sSf | sh -s -- -y --default-toolchain stable --profile minimal
rustup target add aarch64-unknown-linux-musl
sudo apt-get install -y gcc-aarch64-linux-gnu g++-aarch64-linux-gnu

bash build-wsl.sh   # 产物回传到 flclash-api/target/flclash-api-wsl
```

等价的裸命令：

```bash
export CC_aarch64_unknown_linux_musl=aarch64-linux-gnu-gcc
export AR_aarch64_unknown_linux_musl=aarch64-linux-gnu-ar
cargo build --release --target aarch64-unknown-linux-musl --bins
```

产物为静态 ELF；`flclash-api` 约 2.4 MiB（内置 rustls/webpki-roots TLS，不再依赖系统 curl）。`flclash-api/.cargo/config.toml` 固定 `rust-lld` 与 `link-self-contained`。

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
| `binaries` | rustup 官方 SDK（含 aarch64 交叉 gcc 供 ring）+ Go 官方工具链 | `cargo test --locked`；ELF / 静态 / 目标架构 |
| `package` | ImmortalWrt SDK 只组装与签名；四组合并行 | 架构闸门、无 `INTERP`、**恰好 1 个 apk** |
| `release` | 汇总产物、生成 `sha256sums.txt` | **恰好 4 个 apk**，再发布 |

### 触发与产物

- `push` 到 `main`：改动 `luci-app-flclash/**`、`flclash-api/**`、`scripts/**`、workflow 或 `README.md` 时触发构建（不发布）。
- `push` 标签 `v*`：构建并发布（tag 名取 workflow env `PKG_VERSION`）。
- 手动：`gh workflow run build-apk.yml -f publish=true`。

产物命名：

```text
luci-app-flclash_<ver>-r<rel>_<arch>_immortalwrt-25.12.2.apk
luci-app-flclash_<ver>-r<rel>_<arch>_immortalwrt-snapshot.apk
```

每个 Release 固定 5 个资产：4 个单包 apk + `sha256sums.txt`。

## 兼容性与限制

- **固件**：ImmortalWrt **25.12.2** 与 **snapshot**，两者各自产出独立单包；同为 apk 包格式。
- **架构**：`aarch64_cortex-a53`（ARM64-A53）与 `x86_64`（通用 `GOAMD64=v1`）。其他架构需自行用脚本编译，发布矩阵暂不覆盖。
- **静态二进制**：`mihomo`、`flclash-api`、`flclash-core-bridge` 均为静态 ELF，无 glibc / musl 运行时依赖，无 `INTERP`。`flclash-api` 内置 rustls + webpki-roots，HTTPS 出站不依赖系统 CA 或 curl。
- **内核依赖**：透明代理需要设备内核具备 `TUN` 与 `nftables`（ImmortalWrt 默认镜像已带 `firewall4` / `nftables`）。这些 `kmod-*` 未在 25.12 的 apk 源发布，因此**不写入硬依赖**，避免 `apk add` 解依赖失败。
- **硬件卸载**：接管期间 `flow_offloading` / `flow_offloading_hw` / `bridger` 会被自动关闭并禁自启（快路径流绕过 netfilter，REDIRECT 对 LAN TCP 失效）。关闭接管后如需恢复，请手动开启。
- **sing-box 不随包内置**：单包只带 mihomo。sing-box 通过 LuCI / WebUI 的「更新内核」在线安装到 `/usr/bin/sing-box`，再由 `core_type=sing-box` 启用。
- **在线更新依赖网络**：内核更新从 GitHub Releases 下载，设备需能访问 `api.github.com` 与 release 资源；受限网络下可手动放置二进制。
- **安装签名**：构建 SDK 的 apk 签名不在设备默认信任库，需 `--allow-untrusted`。
- **升级**：旧版三包与单包写入相同路径，升级前先 `apk del` 旧三包。
- **安全**：`9098` 默认监听 `0.0.0.0`。在不可信 LAN 中请设置 `flclash.api.secret`，或改为仅监听回环并自行做反代。代理端口 `7893/7894` 在非 LAN 接口被 reject（WAN Proxy Guard）。

## 验证

仓库内可复现的检查：

| 检查 | 方式 | 结果 |
| --- | --- | --- |
| Rust 单元测试 | `cd flclash-api && cargo test --locked` | 全部通过 |
| 二进制架构与静态性 | CI `binaries` / `package` 架构闸门 | ELF、静态、目标架构、无 `INTERP` |
| 单包约束 | `package` job 断言 | 每个组合**恰好 1 个** apk |
| 发布一致性 | `release` job + `sha256sum -c` | 4 个 apk 全部匹配 |

设备端透明代理与 DNS 的复核方法：

```sh
# 1. 状态三处一致：UCI 开关 / 状态文件 / nft 规则
uci get flclash.proxy.enabled               # 用户期望值
cat /tmp/flclash-proxy.state                # on（REDIRECT）/ tun（TUN）/ off
/usr/libexec/flclash/proxy-rules.sh status  # ACTIVE (nftables) / ACTIVE-TUN

# 2. 劫持规则挂在正确的 LAN 接口（旁路由通常是 br-lan）
nft list table inet flclash | grep iifname
#   应见 br-lan 的 53 -> 1053、TCP -> 7893、UDP -> TPROXY 7894

# 3. 无残留 TUN 策略路由（核心死亡会留下 9000-9010 残留）
ip rule | grep -c 2022                      # 接管时应为 0（TUN 模式除外）
nft list tables | grep mihomo               # TUN 模式下存在属正常

# 4. 内核各监听端口就绪（接管时 allow-lan 使 7893/7894 绑定 *）
netstat -lntup | grep -E ':1053|:7893|:7894|:9097'

# 5. LAN 客户端默认网关指向本机，访问外网应出现在连接列表
wget -q -O - http://<设备IP>:9098/flclash/api/connections
```

验证启停状态感知与双栈显示：

```sh
# 核心停止：出口应变为本机直连 WAN IP（20 秒内）
curl -s -X POST http://127.0.0.1:9098/flclash/api/service/stop
wget -q -O - http://127.0.0.1:9098/flclash/api/status | grep -oE '"outboundIp[^,]*'

# 核心启动：接管自动回位（state=on），出口显示代理节点 IP（+ 节点有 v6 时附带 IPv6）
curl -s -X POST http://127.0.0.1:9098/flclash/api/service/start
wget -q -O - http://127.0.0.1:9098/flclash/api/status | grep -oE '"outboundIp[^,]*'
```

管理面（SSH / LuCI）走私网地址，不在劫持范围内，应始终可用；接管期间公网来源的回包由 `ct direction reply` / `ct original daddr private` 规则豁免。

## 目录结构

```text
.
├── .github/workflows/build-apk.yml   # 二进制 → 打包 → 发布
├── flclash-api/                      # Rust crate
│   ├── src/main.rs                   # axum 路由与鉴权
│   ├── src/ipinfo.rs                 # 出口 IP 多源查询（CF 优先 / 双栈 / 汉化 / 启停感知）
│   ├── src/{config,convert,profiles,singbox,loops}.rs
│   └── src/bin/flclash-core-bridge.rs
├── luci-app-flclash/
│   ├── Makefile                      # 唯一 BuildPackage
│   ├── files/                        # 配置、init、脚本、LuCI、rpcd
│   │   └── usr/libexec/flclash/proxy-rules.sh   # 透明代理状态机
│   └── webui/                        # HTML5 WebUI（css / js / views / fonts / img）
├── scripts/
│   ├── build-core.sh                 # Go 内核交叉编译
│   └── verify-core.sh                # 静态性与架构校验
└── README.md
```

WebUI 按视图拆分：`profiles`、`proxies`、`requests`、`rules`、`dns`、`logs`、`settings`、`tools`，共 8 个视图；首页仪表盘展示双栈出口 IP、归属与质量。

## 许可证与致谢

- 本项目：**GPL-3.0-only**，见 [LICENSE](LICENSE)。
- 上游：[FlClash](https://github.com/chen08209/FlClash)（IPC core）、[mihomo](https://github.com/MetaCubeX/mihomo)、[sing-box](https://github.com/SagerNet/sing-box)。

> 本项目不向项目作者收集订阅或设备凭据。为展示出口 IP 归属与质量，运行时会向 `www.cloudflare.com`（cdn-cgi trace）、`ip.api.skk.moe`、`api.ipfinal.com`、`api.ip.sb`、`ipwho.is`、`ip-api.com` 发起查询（会暴露当前出口 IP）；其余数据仅保存在设备本地。
