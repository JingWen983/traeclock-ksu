#!/system/bin/sh
# TraeClock 免重启更新脚本 — 刷入新 zip 后原地换二进制并重启守护进程。
#
# 用法（root shell / KernelSU 管理器终端 / adb shell + su）：
#   sh /data/adb/traeclock/update.sh /data/local/tmp/TraeClock.zip
#
# 原理：框架刷 zip 要重启才挂载新文件；此脚本绕开框架，直接在
# /data/adb/traeclock/update-tmp 解压、校验 ABI、替换当前 daemon 二进制
# 并重启进程。数据目录 /data/adb/traeclock 全程不动（账号/配置/日志保留）。
#
# 安全性：
# - 新 daemon 先启动失败则自动回滚旧二进制并重启，不会让守护进程瘫痪
# - traeclockctl 同步替换（credits/credit-diffs/debug-credits 等命令同步生效）

ZIP="$1"
MODDIR="/data/adb/modules/traeclock"
DATA="/data/adb/traeclock"
BIN_D="$DATA/bin"
TMP="$DATA/update-tmp"
BACKUP="$DATA/update-backup"

say() { echo "[TraeClock] $*"; }

[ "$(id -u)" = "0" ] || { say "需要 root（请用 su）"; exit 1; }
[ -n "$ZIP" ] || { say "用法: sh $0 <TraeClock.zip 路径>"; exit 1; }
[ -f "$ZIP" ] || { say "zip 不存在: $ZIP"; exit 1; }

ABI=$(getprop ro.product.cpu.abi 2>/dev/null)
case "$ABI" in
    arm64-v8a|armeabi-v7a|x86_64) ;;
    *) say "无法识别 ABI: $ABI"; exit 1 ;;
esac
say "设备 ABI: $ABI"

# ── 1. 解压到临时目录 ────────────────────────────────────────────────────────
rm -rf "$TMP"
mkdir -p "$TMP"
if ! (cd "$TMP" && unzip -oq "$ZIP" -x "META-INF*" 2>/dev/null); then
    say "解压失败: $ZIP"
    rm -rf "$TMP"
    exit 1
fi
if [ ! -f "$TMP/bin/$ABI/traeclockd" ] || [ ! -f "$TMP/bin/$ABI/traeclockctl" ]; then
    say "zip 内无 $ABI 二进制（不是 TraeClock 模块包或版本损坏）"
    rm -rf "$TMP"
    exit 1
fi

NEW_VER=$(grep '^version=' "$TMP/module.prop" 2>/dev/null | cut -d= -f2)
OLD_VER=$(grep '^version=' "$MODDIR/module.prop" 2>/dev/null | cut -d= -f2)
say "更新: ${OLD_VER:-未知} → ${NEW_VER:-未知}"

# ── 2. 备份当前二进制 + 模块元文件 ───────────────────────────────────────────
mkdir -p "$BIN_D" "$BACKUP"
for f in traeclockd traeclockctl; do
    [ -f "$BIN_D/$f" ] && cp -f "$BIN_D/$f" "$BACKUP/$f"
done
# metadata rollback source: copy from the CURRENT module dir (these only
# change via this script, so the current copy is always the last good one)
for f in module.prop service.sh customize.sh uninstall.sh; do
    [ -f "$MODDIR/$f" ] && cp -f "$MODDIR/$f" "$BACKUP/$f"
done

# ── 3. 停守护进程 ────────────────────────────────────────────────────────────
OLD_PID=$(cat "$DATA/daemon.pid" 2>/dev/null)
[ -n "$OLD_PID" ] && kill "$OLD_PID" 2>/dev/null
pkill traeclockd 2>/dev/null
sleep 1

# ── 4. 替换二进制 + 模块目录（下次重启自启也用新版） ─────────────────────────
install_fail=0
cp -f "$TMP/bin/$ABI/traeclockd"  "$BIN_D/traeclockd"  || install_fail=1
cp -f "$TMP/bin/$ABI/traeclockctl" "$BIN_D/traeclockctl" || install_fail=1
chmod 700 "$BIN_D/traeclockd" "$BIN_D/traeclockctl" 2>/dev/null
if [ "$install_fail" != "0" ]; then
    say "二进制复制失败，回滚"
    for f in traeclockd traeclockctl; do
        [ -f "$BACKUP/$f" ] && cp -f "$BACKUP/$f" "$BIN_D/$f"
    done
    rm -rf "$TMP"
    exit 1
fi

# module.prop / service.sh / customize.sh 一并更新（下次重启后框架目录也是新版）
cp -f "$TMP/module.prop"  "$MODDIR/module.prop"  2>/dev/null
cp -f "$TMP/service.sh"   "$MODDIR/service.sh"   2>/dev/null
cp -f "$TMP/customize.sh" "$MODDIR/customize.sh" 2>/dev/null
cp -f "$TMP/uninstall.sh" "$MODDIR/uninstall.sh" 2>/dev/null

# ── 5. 启动新 daemon + 健康检查（失败自动回滚） ─────────────────────────────
rm -f "$DATA/daemon.pid"
nohup "$BIN_D/traeclockd" > "$DATA/logs/daemon.out" 2>&1 &
sleep 2
NEW_PID=$(cat "$DATA/daemon.pid" 2>/dev/null)
[ -z "$NEW_PID" ] && NEW_PID=$(pidof traeclockd)
status_probe=$("$BIN_D/traeclockctl" status 2>/dev/null | grep -c '"ok":true')
if [ -z "$NEW_PID" ] || ! [ -d "/proc/$NEW_PID" ] || [ "$status_probe" != "1" ]; then
    say "新 daemon 启动失败，回滚旧版"
    pkill -9 traeclockd 2>/dev/null
    for f in traeclockd traeclockctl; do
        [ -f "$BACKUP/$f" ] && cp -f "$BACKUP/$f" "$BIN_D/$f"
    done
    chmod 700 "$BIN_D/traeclockd" "$BIN_D/traeclockctl" 2>/dev/null
    nohup "$BIN_D/traeclockd" > "$DATA/logs/daemon.out" 2>&1 &
    sleep 2
    # metadata was already replaced from the broken zip — restore all of it
    for f in module.prop service.sh customize.sh uninstall.sh; do
        [ -f "$BACKUP/$f" ] && cp -f "$BACKUP/$f" "$MODDIR/$f"
    done
    say "已回滚并重启旧版 daemon"
    rm -rf "$TMP"
    exit 1
fi

# ── 6. 功能验证 ──────────────────────────────────────────────────────────────
# full output (ok:true sits at the END of the status JSON — never truncate)
STATUS_OUT=$("$BIN_D/traeclockctl" status 2>/dev/null)
if echo "$STATUS_OUT" | grep -q '"ok":true'; then
    say "更新完成 ✓  daemon pid=$NEW_PID  版本 $NEW_VER"
    say "账号/配置/日志均保留（/data/adb/traeclock 未动）"
else
    say "警告: status 校验未通过，但 daemon 已在运行（pid=$NEW_PID），请观察日志"
fi

# ── 7. 清理 ─────────────────────────────────────────────────────────────────
rm -rf "$TMP"
exit 0
