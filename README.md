# luci-app-flclash

FlClash WebUI + mihomo 静态内核 + 同源 API 桥接，面向 OpenWrt 主线（master）与
SNAPSHOT 快照版，目标架构 `aarch64_cortex-a53`。

## 架构

```
浏览器 ──► LuCI「服务 → FlClash」 ──► /etc/init.d/flclash-api  (先起桥接)
                                   ├─► /etc/init.d/flclash-core (再起内核)
                                   └─► 跳转 http://<router>:9098/flclash/

浏览器 ──► 9098 /flclash/        静态 WebUI（bridge 同源提供）
        └─ 9098 /flclash/api/*   Go 桥接 ──► 127.0.0.1:9097 (mihomo controller)
```

- **LuCI 只做两件事**：启动服务、跳转 WebUI。
- **WebUI**：HTML5 + CSS + JS，功能与 FlClash 对齐；不直接接触内核。
- **flclash-api**：stdlib-only 静态 Go 程序，补齐 FlClash 语义并转发 mihomo 原生接口。
- **flclash-core**：静态 mihomo 内核（`CGO_ENABLED=0`、`with_gvisor`），由 procd 托管。

## 包与端口

| 包 | 作用 | 安装内容 |
| --- | --- | --- |
| `flclash-core` | 内核 | `/usr/bin/mihomo`、`/etc/init.d/flclash-core`、`/etc/config/flclash` |
| `flclash-api` | API 桥接 | `/usr/bin/flclash-api`、`/etc/init.d/flclash-api` |
| `luci-app-flclash-webui` | 入口 + 静态页 | menu.d、ACL、launch.js、`/www/flclash/` |

默认监听：

- mihomo controller：`127.0.0.1:9097`（仅回环，不对外）
- 桥接/WebUI：`0.0.0.0:9098`（局域网访问；可选 `api.secret`）
- LuCI 菜单跳转目标：`http://<host>:9098/flclash/`

## 桥接层实现（`flclash-api/main.go`）

仅使用 Go 标准库，单文件：

- 静态资源：`/flclash/` 由 `http.FileServer` 提供，`/` 302 到 `/flclash/`；
- 认证：设置 `api.secret` 后，API 需 `X-FlClash-Secret` 或 `Authorization: Bearer`；
- 后台采样：`/traffic`（速率与历史）、`/logs`（最近 500 条）、`/connections`
  （累计流量），供 `/status` 使用；
- 语义映射：

| FlClash API | 实现 |
| --- | --- |
| `GET /status` | `/version` + `/configs` + `/connections` + 采样缓存 + `/proc` 内存/运行时长 |
| `GET /proxies` | `/proxies` map → `{groups, nodes}` |
| `POST /proxies/select` | `PUT /proxies/{group}` |
| `POST /proxies/delay` | `GET /proxies/{name}/delay` |
| `GET /connections` | 字段映射（chains/rule/upload/download/start） |
| `DELETE /connections/{id}` | `DELETE /connections/{id}` |
| `GET /config` / `PUT /config` | `/configs` 读取 / `PATCH /configs` |
| `GET /logs` | 采样缓存数组 |
| `GET /profiles` | `/etc/flclash/index.json` + `profiles/*.yaml` |
| `POST /profiles/import` | 下载订阅并落盘 |
| `POST /profiles/{id}/apply` | 复制为 `config.yaml` 并 `PUT /configs {path}` |
| `DELETE /profiles/{id}` | 删除索引与文件 |
| `POST /service/{action}` | 调用 `/etc/init.d/flclash-core` |
| `GET /system/proxy` | 只读汇总（端口/模式/IPv6/运行状态） |

未实现/降级：`GET /dns/queries` 返回空数组（mihomo 不提供查询历史）；
`POST /dns/flush` 尽力调用 `/cache/fakeip/flush`。

## UCI

`/etc/config/flclash`：

```uci
config flclash 'main'
    option enabled '1'
    option config_file '/etc/flclash/config.yaml'
    option external_controller '127.0.0.1:9097'
    option mixed_port '7892'
    option mode 'rule'
    option log_level 'info'

config flclash 'api'
    option enabled '1'
    option listen '0.0.0.0:9098'
    option ui_dir '/www/flclash'
    option data_dir '/etc/flclash'
    option secret ''          # 非空时 API 需要请求头校验

config flclash 'webui'
    option enabled '1'
    option webui_path '/flclash/'
    option webui_port '9098'
```

> `api.secret` 同时作为桥接的客户端凭据与转发给 mihomo 的 `Authorization`；
> 若内核侧单独配置了 `secret`，请保持一致，或保持两者为空并依赖 LAN 访问控制。

## 构建

### CI（推荐）

`.github/workflows/build-apk.yml` 会：

1. 拉取 FlClash / Clash.Meta 并编译 `mihomo-arm64`；
2. 在 `flclash-api/` 执行 `go vet ./...` 与静态交叉编译；
3. 下载 OpenWrt SNAPSHOT SDK（`mediatek/filogic`）；
4. 编译三个包并收集 `.apk`（数量少于 3 直接失败）；
5. 上传 artifact 并发布 Release。

### 本地

```bash
./scripts/build-core.sh
./scripts/verify-core.sh ./flclash-core/files/usr/bin

cd flclash-api
CGO_ENABLED=0 GOOS=linux GOARCH=arm64 go build -trimpath -ldflags "-s -w" \
  -o files/usr/bin/flclash-api .
```

## 兼容性

- OpenWrt master / SNAPSHOT（apk）；24.10（opkg）同样适用，包定义未使用 apk 专有语法。
- 内核与桥接均为静态 ELF，无 glibc/musl 运行时依赖。
- 包架构跟随 `ARCH_PACKAGES`；不支持的架构在解析阶段直接报错，不会误装 arm64 二进制。
- `flclash-core` 首启自动生成最小 `config.yaml`，不再因缺配置进入 respawn 循环。
