# TraeClock KernelSU 模块

多账号签到守护进程（Trae / WorkBuddy / Qoder），由 CI 从 `daemon/` 源码交叉编译后按 ABI 装入本目录的 `bin/<abi>/`。

## 安装

在 KernelSU 管理器中刷入 `TraeClock-<version>.zip`。安装脚本会：

1. 校验设备 ABI（arm64-v8a / armeabi-v7a / x86_64）；
2. 将对应 ABI 的 `traeclockd` / `traeclockctl` 复制为 `bin/`；
3. 设置 root-only 权限（0700）；
4. 创建数据目录 `/data/adb/traeclock`（模块升级不丢失）。

重启后 `service.sh` 启动守护进程（防重复拉起）。

## 数据目录

```
/data/adb/traeclock/
├── config.json        # 签到窗口/抖动/间隔/header 覆盖
├── accounts.json      # 账号凭证（仅 root 可读）
├── cooldowns.json     # 错误冷却状态
├── state.json         # 最近一轮结果 / 积分历史
└── logs/              # checkin.log + events.ndjson + daemon.out
```

## 手动管理（adb shell / su）

```sh
su -c /data/adb/modules/traeclock/bin/traeclockctl status
su -c /data/adb/modules/traeclock/bin/traeclockctl run-now trae
```

日常管理请使用 TraeClock 管理器 App（App 内部即调用此 CLI）。

## 免重启更新

刷完整模块 zip 需要重启才能挂载新文件。用免重启更新器可原地换 daemon
（v0.5.9 起随模块自带，首次之后常驻数据目录）：

```sh
# 1. 下载新 TraeClock.zip（GitHub Release 或任意渠道）推到设备
adb push TraeClock.zip /data/local/tmp/

# 2. 免重启更新（root shell）
su -c 'sh /data/adb/traeclock/update.sh /data/local/tmp/TraeClock.zip'
```

脚本行为：解压校验 ABI → 备份当前二进制 → 停 daemon → 替换
`/data/adb/traeclock/bin/` + 模块目录元文件 → 启动新 daemon →
`traeclockctl status` 健康检查；新 daemon 起不来会**自动回滚**旧版并重启。
账号 / 配置 / 日志全程保留。旧版模块（< v0.5.9）先用老方法刷一次 zip 重启，
之后即可一直免重启。

## 卸载

KernelSU 中直接删除模块。`/data/adb/traeclock`（账号与日志）默认保留，
如需彻底清除：`su -c rm -rf /data/adb/traeclock`。
