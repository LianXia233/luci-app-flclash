# luci-app-flclash

FlClash WebUI + mihomo 静态内核 + **Rust/Tokio** 同源 API 桥接,面向 OpenWrt
主线(master)与 SNAPSHOT 快照版,目标架构 `aarch64_cortex-a53`。

功能对齐上游 FlClash(chen08209):订阅导入与自动转换、配置管理(更新 /
编辑 / 自动更新 / 流量与到期)、代理组与节点测速、连接、日志、规则、DNS
进阶(fake-ip / redir-host,DoH / DoT / bootstrap)、仪表盘(IP 资料、地区、
质量)、内核停止后 WebUI 全功能可用、LuCI 内启停开关与内核更新。

## 架构

```
浏览器 ──► LuCI「服务 → FlClash」(服务开关 / 内核更新 / 日志) ──► /etc/init.d/flclash-api
                                   ├─► /etc/init.d/flclash-core (mihomo)
                                   └─► http://<router>:9098/flclash/

浏览器 ──► 9098 /flclash/        静态 WebUI(bridge 同源提供)
        └─ 9098 /flclash/api/*   Rust/Tokio 桥接 ──► 127.0.0.1:9097 (mihomo controller)
```

- **LuCI**:单页(`admin/services/flclash`,服务 / 日志双标签),启停服务、
  更新内核、一键进 WebUI。通过 rpcd ucode 对象 `flclash` 读写,视图内使用
  直连 fetch 调 `admin/ubus`(见 AGENTS.md 的 rpc.js 约束)。
- **WebUI**:HTML5 + CSS + JS(每视图一个文件),Material Symbols 图标字体,
  PC 导航栏 / 手机底部导航,响应式配置抽屉(PC 右侧 / 手机底部,禁遮罩关闭
  + 未保存保护)。
- **flclash-api**:Rust/Tokio(axum)静态 musl 二进制。订阅格式识别与转换
  (Clash YAML / base64 节点列表 / ss ssr vmess vless trojan hysteria2 tuic
  分享链接),配置持久化,DNS 覆写,自动更新,出口 IP 资料 / 质量,内核停止
  时全接口优雅降级。
- **flclash-core**:静态 mihomo 内核(`CGO_ENABLED=0`、`with_gvisor`),procd
  托管;LuCI 可在线更新到上游最新版。

## 包与端口

| 包 | 作用 | 安装内容 |
| --- | --- | --- |
| `flclash-core` | 内核 | `/usr/bin/mihomo`、`/etc/init.d/flclash-core`、`/etc/config/flclash` |
| `flclash-api` | API 桥接(Rust) | `/usr/bin/flclash-api`、`/etc/init.d/flclash-api` |
| `luci-app-flclash-webui` | LuCI 入口 + WebUI | menu、ACL、launch 视图、rpcd 对象、core-update.sh、`/www/flclash/` |

默认监听:mihomo controller `127.0.0.1:9097`(仅回环);桥接/WebUI
`0.0.0.0:9098`(LAN);可选 `api.secret` 请求头校验。

## 桥接层(`flclash-api/`,Rust 单 crate 多模块)

| FlClash API | 实现 |
| --- | --- |
| `GET /status` | 控制器 + config.yaml 文件回退(内核停止仍完整)+ 出口 IP 资料/质量 |
| `GET /proxies` | `{groups, nodes}`;内核停止返回空集(200) |
| `POST /proxies/select` `POST /proxies/delay` `POST /proxies/group-delay` | 组/节点测速与切换 |
| `GET /connections` `DELETE /connections/{id}` | 连接列表与关闭 |
| `GET /config` `PUT /config` | 标量 + `dns` 块;持久化到 config.yaml(重启保留),内核停止仍可保存 |
| `GET /rules` `GET /providers/proxies` `GET /dns/resolve` | 控制器透传 |
| `GET /logs` | 采样缓存(ring buffer) |
| `GET /profiles` `POST /profiles/import` `POST /profiles/{id}/update` | 订阅管理:下载→转换→元数据(节点/组/规则/流量/到期) |
| `POST /profiles/{id}/apply` | 转换→原子写 config.yaml→重载或启动内核 |
| `PATCH /profiles/{id}` | 名称 / URL / 自动更新间隔 |
| `POST /service/{action}` | 内核 init 脚本 |
| 静态 | `/flclash/` 同源提供 WebUI(含 Material Symbols 字体) |

内核停止时的降级契约:读接口 200 + 文件回显;依赖内核的写操作返回明确
503「内核未运行」,不泄漏 dial 错误;`apply` 会自行拉起内核。

## CI

`.github/workflows/build-apk.yml`:cargo test → rust-lld 静态交叉编译
aarch64-musl → 编译静态 mihomo → OpenWrt SNAPSHOT SDK 打包三个 apk →
Release(tag `apk-aarch64-cortex-a53-v5`)。

## 本地构建桥接

```bash
cd flclash-api
cargo test
RUSTFLAGS="-C link-self-contained=yes -C linker=rust-lld" \
  cargo build --release --target aarch64-unknown-linux-musl
# 产物: target/aarch64-unknown-linux-musl/release/flclash-api (~1.1 MB 静态)
```

## 兼容性

- OpenWrt master / SNAPSHOT(apk);24.10(opkg)同样适用。
- 内核与桥接均为静态 ELF,无 glibc/musl 运行时依赖。
- `flclash-core` 首启自动生成最小 `config.yaml`;订阅导入路径保证写入的
  配置永远是内核可加载的 YAML(历史上的 base64 直写崩溃已根治)。
