#!/usr/bin/env bash
# 版本号唯一入口：改这一处，需要手写的两份构建文件同步。
#
#   scripts/bump-version.sh 0.5.22
#
# 版本格式 X.Y.Z（每段 < 100，因为 versionCode 用段位编码）：
#   versionCode = X*10000 + Y*100 + Z      （0.5.21 -> 521，1.2.3 -> 10203）
# 只增不减，保证 Android 覆盖安装。
#
# 谁从哪读版本：
#   VERSION                 ← 唯一来源；app/app/build.gradle.kts 直接读它，
#                             所以 APK 的 versionName/versionCode 永远跟着 VERSION
#   daemon/Cargo.toml       ← 必须写字面量（cargo 不读外部文件）；脚本同步
#   module/module.prop      ← 必须写字面量（KernelSU 面板 + 关于页读它）；脚本同步
#
# CI 的 version-guard 会校验这三处一致；打 tag 时还要求 tag == v$VERSION。
set -euo pipefail

ver="${1:-}"
if [[ ! "$ver" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]]; then
  echo "用法: scripts/bump-version.sh <X.Y.Z>   例如 0.5.22" >&2
  exit 1
fi
ma="${BASH_REMATCH[1]}"
mi="${BASH_REMATCH[2]}"
pa="${BASH_REMATCH[3]}"
for n in "$ma" "$mi" "$pa"; do
  if (( n >= 100 )); then
    echo "每段必须 < 100（versionCode = X*10000 + Y*100 + Z 编码），收到 $ver" >&2
    exit 1
  fi
done
code=$(( ma * 10000 + mi * 100 + pa ))

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

printf '%s\n' "$ver" > VERSION

# daemon/Cargo.toml —— 只改 [workspace.package] 里第一条列首的 version
sed -i -E "0,/^version = \"[^\"]*\"$/s//version = \"$ver\"/" daemon/Cargo.toml

# module/module.prop —— version 带 v 前缀（KernelSU 面板显示用）
sed -i -E "s/^version=.*/version=v$ver/" module/module.prop
sed -i -E "s/^versionCode=.*/versionCode=$code/" module/module.prop

echo "已同步版本 $ver（versionCode $code）:"
echo "  VERSION                -> $ver"
grep -H -m1 '^version = ' daemon/Cargo.toml | sed 's/^/  /'
grep -H -m1 '^version=' module/module.prop | sed 's/^/  /'
grep -H -m1 '^versionCode=' module/module.prop | sed 's/^/  /'
echo "  app 版本由 VERSION 派生，无需改 app/app/build.gradle.kts"
