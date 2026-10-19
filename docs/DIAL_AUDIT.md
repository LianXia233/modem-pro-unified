# 拨号与网口拉起审计（DIAL_AUDIT）

> 审计对象与版本同 [`UPSTREAM_AUDIT.md`](./UPSTREAM_AUDIT.md)（QModem `2ef9227`、
> Go 版 `397e014`）。本文只记**源码核对过**的事实与本轮实际改动；没核到的写「未核实」。

---

## 1. 三方拨号实现对照

| 维度 | QModem（shell） | modem-pro（Go） | 本仓库（Rust） |
| --- | --- | --- | --- |
| 拨号方式判定 | `modem_support.json` 的 `modes` + `modem_dial.sh` 按厂商挑拨号器 | `dial.go` 的 `DialTypes()`（usbnet 取值表）+ USB 描述符兜底 | `model/dial.rs::dial_types()`（同源表）+ `classify_usb_classes()` + 主机驱动 |
| QMI / MBIM | 外部拨号器（`quectel-CM` 等） | 主机侧拨号器，本程序托管 | `runtime/dialer/`（**内置** qmi.rs / mbim.rs），`wan_host_dial_by_daemon()` 分派 |
| ECM / NCM / RNDIS | netifd proto 脚本（`qmodem_runtime.sh`）+ AT+CGACT | AT+CGACT + netifd | 同 Go：`set_pdp()`（含断开回读确认）+ netifd |
| PPP | 有脚本分支 | 无 | 无（数据 PDP 只认 IP/IPV6/IPV4V6，见 `valid_pdp_type`） |
| APN / 认证 | 脚本变量 | DialRequest | `DialRequest{cid,apn,pdp_type,reconnect,force}`，APN 白名单校验 |
| 连接判定 | 拨号器进程在 + 接口有地址 | `DialState` + 地址 | **本轮补齐三层判定**（见 §2） |
| IPv6 | ndisc6 / DHCPv6 脚本 | netprov v6 | `netprov/v6.rs`（V6Plan：dhcpv6 / 静态 / 关闭），PDP 地址分拆 v4/v6 |
| 网口关联 | udev/hotplug 脚本 | `usb_netifs_for_port`（USB 拓扑） | 同 Go + QMAP mux 口优先（`wwan0_1`） |
| 地址获取 | DHCP 或手工 | 模组上报地址 → static（FM350 特例） | `netprov::plan_wan`：350 系列直接 proto=static + 模组地址 |
| 重连 | init 脚本循环 | 拨号器退出自动重拉 | `runtime/dialer` 托管循环 + `manual_stop` 尊重用户 |

结论：**方式判定、按类型分派、网口关联这三块本仓库早已与 Go 上游对齐**（同一套表、
同一套判据，且有实机注释）；真正的缺口在「连接状态分层」与「路由事实」，见 §2。

---

## 2. 本轮发现并修复的问题

### 2.1 「已连接」不分层（本轮修复）

`Modem::dial()` 的 AT 路径（ECM/NCM/RNDIS）收尾只凭两件事就置 `state=connected`：
`AT+CGACT=1` 回 OK + `AT+CGPADDR` 读到地址。但「主机真的能出去」还需要：

3. 主机网卡存在且**拿到了地址**（FM350 实测：模组侧都好，eth2 挂 169.254）；
4. **默认路由经这块网卡**（有地址但路由被别的口抢走，照样打不开网页）。

原来的 `dial_status()` 对 netifd 拉起的链路（`link_source=netifd`）倒是用「有地址」
判过，但同样没看路由。

**修复**：新增 `phase` 阶段层（`state` 契约不变）：

| phase | 含义 | 判据 |
| --- | --- | --- |
| `idle` | 未初始化 | 非 dialing/failed/connected |
| `dialing` | 拨号执行中 | state=dialing |
| `iface_pending` | 会话已建立，主机网卡未就绪 | 网卡缺失或无地址 |
| `route_pending` | 网卡有地址但默认路由不经它 | `has_default_via(iface)==false` |
| `online` | 地址 + 默认路由都有 | 三层全过 |
| `failed` | 失败 | state=failed |

- 路由事实读 `/proc/net/route` + `/proc/net/ipv6_route`（**不 fork**，`netprov/route.rs`，
  6 个单测覆盖小端还原 / 默认路由识别 / 子网路由排除）；
- `dial_status()` 是 HTTP/WS/snapshot 的唯一出口，phase 在那里统一算；
- `dial()` 收尾把「主机网卡就绪」「默认路由经该网口」两条写进 `steps`，
  失败的那条直接给出排障方向；
- LuCI 拨号页加「链路阶段」行，WebUI 拨号卡加阶段徽标（同一份 JSON）。

实机验证（netifd 拉起的 eth2）：

```
state=connected   phase=online（网络就绪（地址 + 默认路由））
host_iface=eth2   host_ips=[10.42.0.16/24（文档示例地址）]
```

`route_pending` / `iface_pending` 两条路径由单测覆盖
（`phase_layers_follow_the_three_facts`，构造三个布尔的所有组合）。

### 2.2 实机遗留路由（记录，非本程序产生）

实机 eth2 上有两条默认路由：`metric 20`（netifd，`proto static`）与
`metric 50`（`proto boot`，启动期由本程序之外的东西写入——本仓库代码里
**没有任何**直接 `ip route add`，只有 hostnic 生成的命令文本）。
这不是本轮引入的，也不由本程序清理（不删别的服务管理的配置）。

---

## 3. 现有机制的澄清（审计中确认过、不需要改的）

| 机制 | 现状 | 出处 |
| --- | --- | --- |
| 重复拨号保护 | `dial_busy` 原子闸：进行中再点直接拒绝 | `dial()` 入口 |
| 重复组网保护 | `diff_plan` 零差异时连 `uci commit` 都不跑 | `netprov/provision.rs` 头注释 |
| 与 netifd 的分工 | 本程序只写 UCI（计划→diff→apply），接口拉起/路由/DHCP 全归 netifd | `netprov/uci.rs` |
| 350 系列静态地址 | 模组上报地址 → `proto=static`，**不**瞎发 DHCP | `plan_wan` + `quirk` |
| QMI/MBIM 不发 CGACT | 模组只有一个数据承载，先激活会把拨号器顶掉（RM502Q 实测） | `dial_inner` ② 注释 |
| netifd 拉起链路的识别 | 用「实际地址」判 connected 并标 `link_source=netifd`，不看 operstate | `dial_status` ③ |
| 断开重连 | 拨号器托管循环重拉；用户手动断开记 `manual_stop`，不再自动拨 | `dial_enable.rs` |

---

## 4. 未做项与理由

| 项 | 理由 |
| --- | --- |
| 自动组网 15s 轮询改事件驱动 | 轮询是兜底（USB 热插拔已记事件，见 `usbdev::note_usb_event`），事件触发会引入新的竞态面；已有「零写入不 reload」的幂等保护，改的收益小、风险大。**待办** |
| DNS 专项诊断 | DNS 归 netifd（`peerdns`/`dns` UCI 项），本程序不直接写 resolv.conf；「有地址无路由」已由 phase 覆盖，DNS 层诊断待实机复现后再加 |
| PPP 数据方式 | 两个上游的数据面也没有把 PPP 当主要路径（QModem 有脚本分支但拨号器生态在 QMI/MBIM/ECM） |
| `route_pending` 的实机复现 | 需要临时改掉实机的默认路由（会影响用户网络），本轮用单测覆盖判定函数 |
