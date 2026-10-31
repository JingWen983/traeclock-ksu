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

# install the no-reboot updater into the data dir (persists across upgrades)
if [ -f "$MODPATH/update.sh" ]; then
    cp -f "$MODPATH/update.sh" /data/adb/traeclock/update.sh
    chmod 700 /data/adb/traeclock/update.sh
    ui_print "- 免重启更新器: /data/adb/traeclock/update.sh"
    ui_print "  下次起直接: su -c 'sh /data/adb/traeclock/update.sh <zip>'"
fi

ui_print "- 安装完成。重启后守护进程自启；使用 TraeClock 管理器 App 管理账号。"

# ── 管理器 App：随模块自动安装 / 升级 ────────────────────────────────────────
# 模块 zip 里带了 manager/TraeClockManager.apk（CI 打包时塞入）。
# 已装且不旧于模块版本则跳过；装了旧版则覆盖升级；未装则安装。
# 这步失败绝不阻塞模块安装，全程容错。
MANAGER_APK="$MODPATH/manager/TraeClockManager.apk"
PKG=cn.traeclock.manager

if [ -f "$MANAGER_APK" ]; then
    MODULE_VC=$(grep '^versionCode=' "$MODPATH/module.prop" | cut -d= -f2)
    INSTALLED_VC=$(dumpsys package $PKG 2>/dev/null | grep -m1 'versionCode=' | sed 's/.*versionCode=\([0-9]*\).*/\1/')

    if [ -n "$INSTALLED_VC" ] && [ "$INSTALLED_VC" -ge "$MODULE_VC" ] 2>/dev/null; then
        ui_print "- 管理器已是最新（versionCode $INSTALLED_VC），跳过"
    else
        if [ -n "$INSTALLED_VC" ]; then
            ui_print "- 更新管理器: versionCode $INSTALLED_VC → $MODULE_VC"
        else
            ui_print "- 安装管理器 App"
        fi
        # 模块目录是 root-only，pm 读不到，先复制到可读路径
        cp -f "$MANAGER_APK" /data/local/tmp/TraeClockManager.apk 2>/dev/null
        chmod 644 /data/local/tmp/TraeClockManager.apk 2>/dev/null
        INSTALL_OUT=$(pm install -r -d /data/local/tmp/TraeClockManager.apk 2>&1)
        case "$INSTALL_OUT" in
            *Success*|*success*) ui_print "- 管理器安装完成" ;;
            *) ui_print "! 管理器自动安装失败（不影响模块），请手动装 release 里的 APK"
               ui_print "  $INSTALL_OUT" ;;
        esac
        rm -f /data/local/tmp/TraeClockManager.apk 2>/dev/null
    fi
else
    ui_print "- 模块未附带管理器 APK，跳过自动安装"
fi
