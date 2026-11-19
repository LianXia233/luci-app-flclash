# luci-app-flclash

FlClash WebUI + 双内核(mihomo / sing-box) + **Rust/Tokio** 同源 API 桥接,面向
OpenWrt 主线(master)与 SNAPSHOT 快照版。目标架构 `aarch64_cortex-a53` 与
通用 `x86_64`,均由静态 musl 交叉编译,内核与桥接均为无运行时依赖的静态 ELF。

订阅一次导入、两种内核无损共用:桥接把任意订阅归一成 mihomo `config.yaml`,
按需再无损转换成 sing-box 原生 JSON(节点/代理组/规则/DNS/TUN 全量等价),
切换内核时监听端口不变。

## 功能

- 订阅导入与自动转换:Clash YAML、base64 节点列表、分享链接
  `ss / ssr / vmess / vless / trojan / hysteria / hysteria2 / tuic / anytls`,
  其中 `anytls` 走 password + 强制 TLS 转换。
- 配置管理(更新 / 编辑 / 自动更新 / 流量与到期)、代理组与节点测速、连接、
  日志、规则、DNS 进阶(fake-ip / redir-host,DoH / DoT / bootstrap)、仪表盘
  (IP 资料 / 地区 / 质量)。
- **双内核切换**:内置 mihomo(原生 IPC core,走 core-bridge)与 sing-box
  1.14(clash_api 模式);WebUI 可在线更新、查看版本、一键切换,切换时自动
  重新转换订阅,端口不动。
- 内核停止后 WebUI 全功能可用(读接口文件回退、写接口优雅降级);LuCI 内
  启停服务、内核更新、日志。

## 架构

```
浏览器 ──► LuCI「服务 → FlClash」(服务开关 / 内核更新 / 日志 / 内核切换) ──► rpcd ubus flclash
                                   ├─► /etc/init.d/flclash-core   (mihomo,IPC)
                                   └─► /etc/init.d/flclash-api    (Rust 桥接)

浏览器 ──► 9098 /flclash/        静态 WebUI(桥接同源提供)
        └─ 9098 /flclash/api/*  Rust/Tokio 桥接 ──► 127.0.0.1:9097 controller
                                        ├─ mihomo   (native IPC core + core-bridge)
                                        └─ sing-box (clash_api,由 config.yaml 无损转换)
```

- **LuCI**:单页(`admin/services/flclash`),服务 / 日志双标签,启停服务、
  更新与切换内核、一键进 WebUI。通过 rpcd ucode 对象 `flclash` 读写,视图
  内用直连 fetch 调 `admin/ubus`。
- **WebUI**:HTML5 + CSS + JS(每视图一个文件),Material Symbols 图标字体,
  PC 导航栏 / 手机底部导航,响应式配置抽屉(PC 右侧 / 手机底部,禁遮罩关闭
  + 未保存保护)。
- **flclash-api**:Rust/Tokio(axum)静态 musl 二进制。订阅识别与转换、
  mihomo→sing-box 无损转换、配置持久化、DNS 覆写、自动更新、出口 IP 资料 /
  质量、内核停止时全接口优雅降级。
- **flclash-core**:静态 mihomo 内核(`CGO_ENABLED=0`、`with_gvisor`,
  `GOARM64=v8.0` / `GOAMD64=v1`),procd 托管,经 flclash-core-bridge 走
  原生 IPC(core.init/setup/listener.start);可在线更新到上游最新版。

## 包与端口

| 包 | 作用 | 安装内容 |
| --- | --- | --- |
| `flclash-core` | mihomo 内核 + IPC 桥 | `/usr/bin/mihomo`、`/usr/bin/flclash-core-bridge`、`/etc/init.d/flclash-core`、`/etc/config/flclash` |
| `flclash-api` | API 桥接 + sing-box 转换(Rust) | `/usr/bin/flclash-api`、`/etc/init.d/flclash-api` |
| `luci-app-flclash-webui` | LuCI 入口 + WebUI | menu、ACL、launch 视图、rpcd 对象、core-update.sh、`/www/flclash/` |

默认监听:mihomo / sing-box controller `127.0.0.1:9097`、mixed `7892`
(切换内核端口不变);桥接 / WebUI `0.0.0.0:9098`(LAN);可选 `api.secret`
请求头校验。sing-box 由 `/usr/libexec/flclash/singbox-convert.sh` 从
`/etc/flclash/config.yaml` 生成 `/tmp/flclash-singbox.json`。

## 桥接层(`flclash-api/`,Rust 单 crate 多模块)

| FlClash API | 实现 |
| --- | --- |
| `GET /status` `GET /version` `GET /memory` | 控制器 + config.yaml 文件回退(内核停止仍完整)+ 出口 IP 资料/质量 |
| `GET /proxies` | `{groups, nodes}`;内核停止返回空集(200) |
| `POST /proxies/select` `POST /proxies/delay` `POST /proxies/group-delay` | 组/节点测速与切换 |
| `GET /connections` `DELETE /connections/{id}` | 连接列表与关闭 |
| `GET /config` `PUT /config` `PATCH /config` | 标量 + `dns` 块;持久化到 config.yaml(重启保留),内核停止仍可保存 |
| `GET /rules` `GET /providers/proxies` `GET /dns/resolve` `GET /dns/queries` `POST /dns/flush` | 控制器透传 / DNS 查询与刷新 |
| `GET /logs` | 采样缓存(ring buffer) |
| `GET /system_proxy` `GET /proxy` `POST /proxy/{action}` | 透明代理状态与开关 |
| `GET /profiles` `POST /profiles/import` `POST /profiles/{id}/update` | 订阅管理:下载→转换→元数据(节点/组/规则/流量/到期) |
| `POST /profiles/{id}/apply` | 转换→原子写 config.yaml→重载或启动内核 |
| `PATCH /profiles/{id}` `DELETE /profiles/{id}` | 名称 / URL / 自动更新间隔 / 删除 |
| `POST /service/{action}` | 内核 init 脚本 |
| 静态 | `/flclash/` 同源提供 WebUI(含 Material Symbols 字体) |

内核停止时的降级契约:读接口 200 + 文件回显;依赖内核的写操作返回明确
503「内核未运行」,不泄漏 dial 错误;`apply` 会自行拉起内核。

### rpcd `flclash` 对象(LuCI 侧)

`system_proxy`(只读系统代理状态)、`service_state` / `service_toggle`(启停)、
`core_log`、`core_update` / `core_update_status`(在线更新)、`core_type` /
`core_switch`(内核查询与切换,支持 `mihomo | sing-box`)。

## CI

`.github/workflows/build-apk.yml` 两个并行 job,均为 cargo test → rust-lld 静态
musl 交叉编译 → 静态内核 → OpenWrt SNAPSHOT SDK 打包三个 apk → 发布 Release:

| job | 目标 | Rust target | 内核 | Release tag |
| --- | --- | --- | --- | --- |
| build | aarch64_cortex-a53 (mediatek/filogic) | `aarch64-unknown-linux-musl` | `GOARM64=v8.0` | `apk-aarch64-cortex-a53-v1.0.0` |
| build-x86 | x86_64 generic (x86/64) | `x86_64-unknown-linux-musl` | `GOAMD64=v1` | `apk-x86-64-v1.0.0` |

## 本地构建桥接

`flclash-api/.cargo/config.toml` 已固定 musl 静态链接参数,直接指定 target 即可:

```bash
cd flclash-api
cargo test
cargo build --release --target aarch64-unknown-linux-musl --bins   # ARM64 / A53
cargo build --release --target x86_64-unknown-linux-musl --bins    # x86_64
# 产物: target/<target>/release/flclash-api (~1.1 MB 静态)
```

核心静态交叉编译见 `scripts/build-core.sh`。

## 兼容性

- OpenWrt master / SNAPSHOT(apk);24.10(opkg)同样适用。
- 内核与桥接均为静态 ELF,无 glibc/musl 运行时依赖。
- 架构:ARM64 / ARM64-A53(`aarch64`,`GOARM64=v8.0`)与 x86_64
  (`GOAMD64=v1`,通用);按设备实际架构选择对应 Release 的三个 apk。
- `flclash-core` 首启自动生成最小 `config.yaml`;订阅导入路径保证写入的
  配置永远是内核可加载的 YAML。

## 实测验证

在 OpenWrt / ImmortalWrt 最新快照虚拟机上按设备架构编译安装后验证:

- 两种内核并存且可切换,mixed / controller 端口不变;WebUI 全标签可读状态、
  调用内核、启停与切换节点,端到端请求全 200、零控制台错误。
- 真实订阅全量节点无损转换成 sing-box 配置;mihomo 与 sing-box 的规则分流
  均按域名命中预期出口(国内直连、境外走代理),TUN 入站可承载流量。
- 内核停止时 WebUI 仍可读状态、可保存配置,代理相关写操作返回明确降级提示。

> 复现脚本与逐项证据见仓库外测试工程;本文不记录任何订阅、出口 IP 或设备凭据。
