#!/usr/bin/env bash
# 새 버전 배포 준비: 빌드 → dist/에 exe 복사 → dist/version.txt 작성 → README SHA256 갱신.
# 사용법: Cargo.toml과 app.manifest의 version을 올린 뒤  tools/release.sh  실행 → 결과를 커밋·push.
# 앱은 dist/version.txt를 하루 한 번 확인해서 새 버전이 있으면 알림만 띄운다(설치는 사용자가 직접).
set -euo pipefail
cd "$(dirname "$0")/.."

OUT="${RBT_DIST_DIR:-dist}"   # 테스트용으로 다른 폴더에 만들 수 있게
VER=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
cargo build --release --target x86_64-pc-windows-gnu
mkdir -p "$OUT"
cp target/x86_64-pc-windows-gnu/release/razer_battery_tray.exe "$OUT/"
printf '%s\n' "$VER" > "$OUT/version.txt"

if [ "$OUT" = "dist" ]; then
    SHA=$(sha256sum "$OUT/razer_battery_tray.exe" | cut -d' ' -f1 | tr a-f A-F)
    sed -i -E "s/^SHA256  [0-9A-F]{64}$/SHA256  $SHA/" README.md
fi
echo "버전 $VER 준비됨: $OUT/razer_battery_tray.exe, $OUT/version.txt"
