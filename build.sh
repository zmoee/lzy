#!/usr/bin/env bash
# 一键构建：先把前端塞进 web/dist，再编出单文件可执行程序。
#
# Windows 版用 MSVC 目标 + cargo-xwin 从 Linux 交叉编译 —— 必须走 MSVC：
# GNU 目标会让 webview2-com-sys 生成对 WebView2Loader.dll 的动态依赖，
# exe 在别人机器上一启动就报"找不到 WebView2Loader.dll"。
#
# 一次性环境准备：
#   cargo install cargo-xwin
#   apt install clang nasm lld
#   （llvm-lib / llvm-rc 也要在 PATH 里，一般随 clang/llvm 装好）
set -euo pipefail
cd "$(dirname "$0")"

export PATH="$HOME/.cargo/bin:$PATH"
TARGET="${1:-all}"

# 图标 icon.ico 是随源码一起带的（从 assets/zmoee.svg 生成好的）。
# 万一丢了，用任何 SVG→ICO 工具重新转一个放回去就行，构建本身不依赖转换脚本。
if [ ! -f assets/icon.ico ]; then
  echo "  ✗ assets/icon.ico 不在。它是随源码带的，用任何 SVG→ICO 工具从"
  echo "    assets/zmoee.svg 转一个出来放回去再构建。"
  exit 1
fi

echo "[1/3] 构建前端 ..."
if [ ! -d web/node_modules ]; then
  (cd web && pnpm install)
fi
(cd web && pnpm build)

mkdir -p dist

if [ "$TARGET" = "all" ] || [ "$TARGET" = "windows" ]; then
  echo "[2/3] 交叉编译 Windows（MSVC）..."
  cargo xwin build --release --target x86_64-pc-windows-msvc
  cp -f target/x86_64-pc-windows-msvc/release/lzy.exe "dist/无限网盘.exe"

  # 自检 1：绝不能有需要用户额外安装的运行时依赖
  if objdump -p "dist/无限网盘.exe" 2>/dev/null | grep -qiE 'WebView2Loader|VCRUNTIME|MSVCP'; then
    echo "  ✗ 产物仍依赖外部 DLL，检查 .cargo/config.toml 里的 crt-static"
    exit 1
  fi
  echo "  ✓ 无外部运行时依赖"

  # 自检 2：图标要编进资源段（.rsrc），否则资源管理器里是默认图标
  if objdump -h "dist/无限网盘.exe" 2>/dev/null | grep -q '\.rsrc'; then
    echo "  ✓ 图标已编入资源段"
  else
    echo "  ✗ 没有 .rsrc 段 —— 图标没编进去，检查 assets/icon.ico"
    exit 1
  fi

  # 自检 3：必须是 GUI 子系统，否则双击会弹终端
  if python3 -c "
import struct,sys
d=open('dist/无限网盘.exe','rb').read()
pe=struct.unpack_from('<I',d,0x3c)[0]
sys.exit(0 if struct.unpack_from('<H',d,pe+0x5c)[0]==2 else 1)
"; then
    echo "  ✓ GUI 子系统（双击不弹终端）"
  else
    echo "  ✗ 不是 GUI 子系统，双击会弹控制台窗口"
    exit 1
  fi
fi

if [ "$TARGET" = "all" ] || [ "$TARGET" = "linux" ]; then
  echo "[2/3] 编译 Linux ..."
  cargo build --release
  cp -f target/release/lzy dist/lzy
fi

echo "[3/3] 完成："
ls -lh dist/
