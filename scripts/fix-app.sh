#!/bin/bash
# 构建后把前端 dist 补进 .app
# 背景：tauri bundler 在本机偶发不把 frontendDist 复制进 .app（Resources 缺 dist 导致空白/旧界面）。
# 用法：tauri build 后执行 `sh scripts/fix-app.sh`
# 注意：.app 目录名来自 tauri.conf.json 的 productName（当前为 "WorkBuddy Shift.app"），
# 不能写死旧 slug；这里用 glob 定位，兼容 debug/release 与 target 位置差异。
set -e
cd "$(dirname "$0")/.." || exit 1

if [ ! -d dist ]; then
  echo "fix-app: 未找到 dist（先运行 pnpm run build）"
  exit 0
fi

for profile in debug release; do
  APP=""
  for candidate in "target/$profile/bundle/macos/"*.app "src-tauri/target/$profile/bundle/macos/"*.app; do
    if [ -d "$candidate" ]; then
      APP="$candidate"
      break
    fi
  done
  if [ -n "$APP" ]; then
    rm -rf "$APP/Contents/Resources/dist" 2>/dev/null || true
    cp -R dist "$APP/Contents/Resources/dist"
    echo "fix-app: dist 已复制到 $APP/Contents/Resources/dist"
    # 签名让 TCC（App 管理/完全磁盘访问）能识别本 app：
    # 优先自签名证书（~/.workbuddy-shift，已加入钥匙串并设为 codeSign 信任），失败回退 adhoc。
    SIGN_IDENTITY="workbuddy-shift Development Signing"
    if codesign --force --deep --sign "$SIGN_IDENTITY" "$APP" 2>/dev/null; then
      echo "fix-app: 已用证书签名 $APP"
    elif codesign --force --deep --sign - "$APP" 2>/dev/null; then
      echo "fix-app: 已 adhoc 签名 $APP（证书不可用）"
    else
      echo "fix-app: 签名失败（跳过，不影响运行）"
    fi
  fi
done
