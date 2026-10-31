# TraeClock · KernelSU 签到模块 + 管理器

[![build](https://github.com/OWNER/traeclock-ksu/actions/workflows/build.yml/badge.svg)](../../actions)

把 [TraeWorkAssistant](https://github.com/smart-open/TraeWorkAssistant)（Windows 桌面端）的
**多账号签到 + 防风控**能力移植到 Android：一个 KernelSU 模块承载常驻签到守护进程
（低占用、低功耗），一个独立的管理器 App 负责账号管理（三应用纯网页登录）。

> ⚠️ **免责声明（使用前必读）**
>
> 1. 本项目与 Trae / Trae Work / WorkBuddy / CodeBuddy / Qoder 官方**无任何关联**，系个人学习研究项目。
> 2. 使用本工具**可能违反相关服务条款**；由此产生的一切后果（包括但不限于账号封禁、积分清零、功能限制）**由使用者自行承担**。
> 3. 请**仅管理本人合法持有的账号**，遵守所在地法律法规。
> 4. 「防风控」指：账号绑定的确定性设备指纹（不漂移）、低频签到（每日一次 + 随机窗口）、错误冷却退避——**不是**伪造他人身份或绕过实名体系。
> 5. 源项目采用 MIT License（© 朱天伟），本移植项目沿用 MIT 并保留原版权声明。

## 组成

| 部分 | 目录 | 说明 |
|---|---|---|
| 协议库 | `daemon/crates/tc-core` | 三应用签到/防风控/OAuth 纯 Rust 移植，77 个单元测试 |
| 守护进程 | `daemon/crates/tc-ctl` | `traeclockd`（调度守护）+ `traeclockctl`（管理 CLI） |
| 模块 | `module/` | KernelSU 模块模板（module.prop / service.sh / customize.sh） |
| 管理器 App | `app/` | Kotlin + Compose，root 通道调用 `traeclockctl` |
| CI | `.github/workflows/build.yml` | 三 ABI 交叉编译 + 模块 zip + APK + Release |

## 支持

- **Trae**（api.trae.cn）：status 预检 → claim → 错误冷却（PlanLimit/SoftRate/SessionDead/Server/Client/AuthRejected/DeviceSigned 分级）、积分三层兜底、确定性设备指纹（device_id/session_id/market_user_id 按账号派生不漂移）、OAuth 网页登录（PKCE + loopback 回调）+ refresh_token 懒续期（≤48h 才轮换）+ ExchangeToken DeviceProof（ECDSA P-256）
- **WorkBuddy / CodeBuddy**（codebuddy.cn）：双域名探测、已签容错、401 刷新一次重试、上游失败详情（error/error_description）透传、奖励宽容解析 + 余额差值兜底、机器/会话双指纹、扫码 OAuth（copilot.tencent.com state 流）
- **Qoder**（openapi.qoder.com.cn）：每日双活动（签到+登录奖励）、设备流 OAuth（nonce 回验 + 兼容模式自动回退，**直连 SSO 登录页**绕开授权页 `:443` 回调 400）、401 刷新一次重试 + 需重登标记、刷新响应兼容 `device_token`/`expires_at`（RFC3339）、每账号稳定设备档案（**档案永不重新生成**——派生指纹归独立桶会 503）
- **豆包**：桌面端 cookie 解密功能**不可移植**，未包含。

> **上游同步**：后端（签到 / 积分 / 凭证 / 风控）逻辑对齐 `smart-open/TraeWorkAssistant` **v3.8.6**。已移植的相关上游修复：#76 设备绑定上限转译、#110 设备当日已签、#114 claim 最小头家族、#118 刷新失败详情透传、#128 服务端拒绝认证（AuthRejected）、#132 campaigns 空表最小头复取、#140 Qoder 直连 SSO 登录页、#141 空表盲发试领兜底、#143 Qoder 续期兼容 `device_token`/`expires_at`。桌面端专有链路（vault/DPAPI、跨进程轮次锁、LLM 网关指纹清洗、通知渠道）不在移植范围。

## 低功耗设计

- 守护进程**无 wake lock**，主循环即「算下次时刻 → nanosleep」；深度睡眠推迟定时器，醒来补签
- RSS 目标 < 5MB（release + opt-level=z + lto + strip），CPU 占用≈0
- 每日签到窗口（默认 08:00–10:00）内**按天确定性抖动选点**，三应用选点互不相同
- 断网退避 15 分钟、每轮最多 3 次重试；网络类失败不杀凭证

## 构建（CI 自动）

推送到 GitHub 即出产物（Actions artifact）；打 `v*` tag 自动发 Release：

- `TraeClock.zip` — KernelSU 模块（arm64-v8a / armeabi-v7a / x86_64）
- `app-release-signed.apk` — 管理器（配置 `TRAECLOCK_KEYSTORE_B64` 等 secrets 后自动签名，否则为未签名包）

### 版本号

唯一来源是仓库根的 `VERSION`，改版本只跑一条命令：

```bash
scripts/bump-version.sh 0.5.22
```

- `versionCode = X*10000 + Y*100 + Z`（0.5.21 → 521），每段必须 < 100
- `app/app/build.gradle.kts` 的 `versionName`/`versionCode` 直接读 `VERSION`，不需要改
- 脚本同步 `daemon/Cargo.toml` 与 `module/module.prop`（这两处 cargo / KernelSU 只能读字面量）
- CI 的 `version-guard` job 会校验三处一致，并检查打 tag 时 `tag == v$VERSION`；不一致直接挡住构建
- 管理器「关于」页显示的版本是**运行时读取**的（APK 走 PackageManager，模块读 `module.prop`），不是写死的字符串

## 安装与使用

1. KernelSU 管理器刷入 `TraeClock.zip` → 重启（守护进程自启）。
   - **管理器 App 会自动装好**：zip 里带了 `manager/TraeClockManager.apk`，
     安装时比对 versionCode —— 没装就装、装的是旧版就覆盖升级、已是最新则跳过。
     刷模块 = 模块 + 管理器一起更新，不用再单独下载 APK。
2. 打开「TraeClock」→ 授予 root。
3. 「账号」页右下角 + 添加账号（三应用都是网页登录，无需电脑）。
4. 「主页」确认下次签到时间；需要立即签点「立即签到（全部）」。

### 自动更新（KernelSU / Magisk）

`module.prop` 的 `updateJson` 指向
`https://raw.githubusercontent.com/JingWen983/traeclock-ksu/main/update.json`，
模块管理器里点「检查更新」即可拉到最新版 zip。

- `update.json` 由 CI 在打 tag 发版时**自动按 `VERSION` 生成并回提交**到 main，
  清单版本不需要手改，不会和 Release 漂移
- 只想更守护进程、不想重启：`su -c 'sh /data/adb/traeclock/update.sh <zip>'`

## 数据与隐私

- 全部数据在设备本地 `/data/adb/traeclock/`（仅 root 可读），**不上传任何服务器**。
- JWT / token 绝不进日志与事件流（源项目的脱敏红线完整保留）。
- 卸载模块默认保留数据目录，彻底清除：`su -c rm -rf /data/adb/traeclock`。

## License

MIT（沿用上游 TraeWorkAssistant 的授权与版权声明；派生项目说明：以
[smart-open/TraeWorkAssistant](https://github.com/smart-open/TraeWorkAssistant) 为基础移植）。


## 许可

本项目管理器 App 的液态玻璃底栏组件移植自 [HyperNavBar](https://github.com/HyperNavBar/HyperNavBar)（其又移植自 SukiSU-Ultra），该部分以 **GPL-3.0** 授权；因此本项目整体以 **GPL-3.0** 发布（原 MIT 许可的其余历史版本不受影响）。
