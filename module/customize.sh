#!/system/bin/sh
# KernelSU / Magisk install-time customization.
#
# NOTE: the framework's zip extraction does NOT preserve unix permission bits
# (binaries land as 644), so all existence checks use -f and every permission
# is (re)applied here via set_perm — never rely on -x before set_perm runs.

SKIPUNZIP=0

ui_print "- TraeClock: 多账号签到模块 (Trae / WorkBuddy / Qoder)"
ui_print "- 数据目录: /data/adb/traeclock"

# ABI gate: the module ships one binary set per ABI directory
ABI=$(getprop ro.product.cpu.abi 2>/dev/null || echo unknown)
ui_print "- 设备 ABI: $ABI"

case "$ABI" in
    arm64-v8a|armeabi-v7a|x86_64)
        if [ ! -f "$MODPATH/bin/$ABI/traeclockd" ] || [ ! -f "$MODPATH/bin/$ABI/traeclockctl" ]; then
            ui_print "! 未找到 $ABI 的守护进程二进制，安装失败"
            ls -la "$MODPATH/bin" "$MODPATH/bin/$ABI" 2>/dev/null
            abort
        fi
        ;;
    *)
        ui_print "! 不支持的 ABI: $ABI（支持 arm64-v8a / armeabi-v7a / x86_64）"
        abort
        ;;
esac

# flatten the per-ABI binaries into bin/
cp -f "$MODPATH/bin/$ABI/traeclockd" "$MODPATH/bin/traeclockd"
cp -f "$MODPATH/bin/$ABI/traeclockctl" "$MODPATH/bin/traeclockctl"
rm -rf "$MODPATH/bin/arm64-v8a" "$MODPATH/bin/armeabi-v7a" "$MODPATH/bin/x86_64"

# permissions: root-only (exec bits come from HERE, not from the zip)
set_perm_recursive "$MODPATH" 0 0 0755 0755
set_perm "$MODPATH/bin/traeclockd" 0 0 0700
set_perm "$MODPATH/bin/traeclockctl" 0 0 0700
set_perm "$MODPATH/service.sh" 0 0 0755
set_perm "$MODPATH/uninstall.sh" 0 0 0755

# user data dir (survives module upgrades)
mkdir -p /data/adb/traeclock/logs
chmod 700 /data/adb/traeclock

ui_print "- 安装完成。重启后守护进程自启；使用 TraeClock 管理器 App 管理账号。"
