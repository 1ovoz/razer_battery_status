#!/usr/bin/env bash
# 새 버전 배포 준비: 빌드 → dist/에 exe 복사 → 서명한 dist/latest.txt 작성 → README SHA256 갱신.
# 사용법: Cargo.toml의 version을 올린 뒤  tools/release.sh  실행 → 결과를 커밋·push.
# 앱은 dist/latest.txt를 하루 한 번 확인하고, 서명이 맞는 새 버전만 받아 스스로 교체한다.
#
# 서명 키(ECDSA P-256 비밀 키)는 저장소에 넣지 않는다. 잃어버리면 자동 업데이트를 더 낼 수 없다(백업할 것).
set -euo pipefail
cd "$(dirname "$0")/.."

KEY="${RBT_SIGNING_KEY:-$HOME/.config/razer_battery_tray/update_signing_key.pem}"
OUT="${RBT_DIST_DIR:-dist}"   # 테스트용으로 다른 폴더에 만들 수 있게
[ -f "$KEY" ] || { echo "서명 키가 없습니다: $KEY" >&2; exit 1; }

VER=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
cargo build --release --target x86_64-pc-windows-gnu
mkdir -p "$OUT"
cp target/x86_64-pc-windows-gnu/release/razer_battery_tray.exe "$OUT/"
SHA=$(sha256sum "$OUT/razer_battery_tray.exe" | cut -d' ' -f1)

# 서명 대상: "버전\nSHA256\n" (버전까지 묶어서 옛 exe를 새 버전으로 속이는 것도 막음)
MSG=$(mktemp)
trap 'rm -f "$MSG"' EXIT
printf '%s\n%s\n' "$VER" "$SHA" > "$MSG"
# openssl은 DER 서명을 내므로 Windows BCrypt가 받는 r||s(64바이트) 형식으로 바꾼다
SIG=$(openssl dgst -sha256 -sign "$KEY" "$MSG" | python3 -I -c '
import sys
d = sys.stdin.buffer.read()
assert d[0] == 0x30
i = 2 if d[1] < 0x80 else 2 + (d[1] & 0x7F)
def read_int(i):
    assert d[i] == 2
    n = d[i + 1]
    return d[i + 2:i + 2 + n], i + 2 + n
r, i = read_int(i)
s, i = read_int(i)
fix = lambda v: v.lstrip(b"\0").rjust(32, b"\0")
print((fix(r) + fix(s)).hex())
')
printf '%s\n%s\n%s\n' "$VER" "$SHA" "$SIG" > "$OUT/latest.txt"

if [ "$OUT" = "dist" ]; then
    UPPER=$(echo "$SHA" | tr a-f A-F)
    sed -i -E "s/^SHA256  [0-9A-F]{64}$/SHA256  $UPPER/" README.md
fi
echo "버전 $VER 준비됨: $OUT/razer_battery_tray.exe, $OUT/latest.txt"
