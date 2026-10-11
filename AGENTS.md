# AGENTS.md — 이 프로젝트를 이어서 작업하는 AI(GPT/Codex 등)를 위한 안내

이 파일은 Claude Code로 진행한 작업을 다른 AI가 이어받을 수 있게 정리한 것이다.
사용자 기능 설명은 `README.md`, 이 파일은 **작업 규칙·현재 상태·주의점**이다.

## 1. 반드시 지킬 규칙 (사용자가 직접 정한 것)

1. **사용자에게는 항상 한국어로 답한다.** 코드·경로·명령은 그대로 둔다.
2. **리소스 최소화.** CPU·메모리·스레드·디스크 I/O를 최소로.
   - Win32 API를 `windows-sys`로 직접 호출한다. Electron, WebView, GUI 프레임워크, 무거운 crate는 쓰지 않는다.
   - 화면은 내용이 바뀔 때만 다시 그린다. 버퍼는 쓰고 나서 해제한다.
   - 기능을 바꾸면 **개인 작업 집합(작업 관리자 "메모리" 열)을 실측해서 보고**한다. 현재 약 1.1 MB.
3. **뱅가드 같은 안티치트/보안 프로그램에 걸릴 일을 절대 하지 않는다.**
   - 하지 않는 것: DLL 주입, 전역 후킹(`SetWindowsHookEx`), 다른 프로세스 메모리 읽기·쓰기, 드라이버, 장치에 직접 HID 명령 보내기, 키보드·마우스 입력 흉내, 게임 위 오버레이, 패커·난독화
   - 데이터는 수동적인 출처(로그 파일)와 공식 API(XInput)에서만 얻는다.
   - 위젯은 항상 z-order 맨 아래에 둔다(다른 창 위로 올라오지 않음).
4. **블루투스 배터리 읽기는 하지 않는다.** 사용자가 거절했다.
5. **자동 업데이트(자동 다운로드·설치)를 다시 넣지 않는다.** 1.2.0부터 "새 버전 알림만" 한다.
   - 서명 키 관리 부담과 "키 + GitHub 동시 유출 시 악성 exe 자동 배포" 위험 때문에 사용자가 결정했다.
6. 큰 결정, 되돌리기 어려운 작업(파일 삭제, 강제 push, 저장소 설정 변경)은 사용자에게 먼저 묻는다.

## 2. 프로젝트 요약

Razer 무선 기기(Synapse 4 로그로 잔량 확인)와 GameSir G7 Pro(XInput)의 배터리를
**트레이 아이콘 + 바탕화면 위젯**으로 보여 주는 Windows 프로그램. Rust, 단일 exe(약 390 KB).

- 저장소: https://github.com/1ovoz/razer_battery_status (공개), 기본 브랜치 `main`
- 현재 버전: **1.2.1** (태그 `v1.0.0` ~ `v1.2.1`)
- 배포 파일: `dist/razer_battery_tray.exe`, `dist/version.txt`(새 버전 알림용 버전 번호 한 줄)
- 사용자 설치 위치(사용자 PC): `%LOCALAPPDATA%\Programs\RazerBatteryTray\razer_battery_tray.exe`
- 설정 파일: `%LOCALAPPDATA%\RazerBatteryTray\settings.ini`

## 3. 코드 구조

| 파일 | 역할 |
|---|---|
| `src/parser.rs` | Synapse 로그 한 줄 해석, 기기 상태. **추적 기기 목록 `TRACKED`** (기기 추가는 여기 한 줄) |
| `src/watcher.rs` | `systray_systrayv2*.log` 여러 파일을 오프셋 기반으로 이어 읽기, 시각순 정렬, 한 줄 최대 1 MB |
| `src/gamepad.rs` | XInput 상태 해석(4단계), 툴팁 |
| `src/config.rs` | settings.ini 읽기·쓰기, 잔량 부족 알림 주기(`Reminder`) |
| `src/update_info.rs` | version.txt 해석, 버전 비교 |
| `src/tray.rs` | 메인: 트레이 아이콘, 메시지 루프, 앱 상태, 새 버전 알림 처리 (Win32) |
| `src/widget.rs` | 바탕화면 위젯(레이어드 창, 픽셀 직접 그리기, 호버 패널, 배치·크기) (Win32) |
| `src/settings.rs` | 설정 창(어두운 테마 Win32 컨트롤, 자동 실행 레지스트리 HKCU Run) |
| `src/syslib.rs` | xinput1_4/uxtheme/dwmapi를 System32에서만 동적으로 불러오기(DLL 바꿔치기 방지) |
| `src/updater.rs` | 새 버전 확인. **같은 exe를 `--update-check`로 띄운 짧은 프로세스에서** WinHTTP로 version.txt만 읽음 |
| `src/main.rs` | `#![windows_subsystem = "windows"]`, Windows 아니면 빈 main |
| `build.rs` | app.manifest(Win10/11 호환, PerMonitorV2 DPI, Common Controls 6) + VERSIONINFO 리소스 삽입 |
| `tests/real_logs.rs` | 실제 로그 폴더로 하는 테스트 (`RAZER_LOG_DIR`로 위치 지정 가능) |
| `tools/release.sh` | 배포 준비: 빌드 → dist 복사 → dist/version.txt → README SHA256 갱신 |
| `tools/gamepad_diag.ps1` | 게임패드 배터리 진단(읽기 전용) |

추적 기기(`TRACKED`): 190 DeathAdder V4 Pro(마우스), 1365 BlackShark V2 Pro(헤드셋), 122 Viper Ultimate(마우스). 4번째 장치는 GameSir(XInput).

## 4. 개발 환경과 명령 (WSL2 Ubuntu에서 Windows용으로 교차 빌드)

- Rust: `~/.cargo/bin` (rustup, 타깃 `x86_64-pc-windows-gnu`)
- MinGW 링커: `~/.local/mingw/usr/bin` (sudo 없이 deb를 풀어 둔 것, `.cargo/config.toml`이 `x86_64-w64-mingw32-gcc-posix` 사용)

```bash
export PATH=~/.cargo/bin:~/.local/mingw/usr/bin:$PATH
cargo test                                                    # 파서·설정 등 (Linux에서도 실행됨)
cargo clippy --release --target x86_64-pc-windows-gnu         # 경고 0개 유지
cargo build --release --target x86_64-pc-windows-gnu          # → target/x86_64-pc-windows-gnu/release/razer_battery_tray.exe
```

Windows 쪽 확인(WSL에서 `powershell.exe` 호출):
- 실행 중이면 exe를 덮어쓸 수 없으니 `Stop-Process -Name razer_battery_tray` 후 복사한다.
- 메모리 측정: `(Get-Counter "\Process(razer_battery_tray)\Working Set - Private").CounterSamples[0].CookedValue/1KB`

**새 버전 배포**:
1. `Cargo.toml`의 `version`과 `app.manifest`의 `RazerBatteryTray` version을 올린다.
2. `tools/release.sh`를 실행한다.
3. README "버전 기록"을 작성한다.
4. 커밋 → 태그 `vX.Y.Z` → push(`--follow-tags`).

push는 사용자의 VS Code GitHub 로그인으로 해 왔다. 다른 AI는 사용자에게 VS Code "소스 제어"에서 동기화를 눌러 달라고 하면 된다.

커밋 메시지는 한국어로 쓴다.

## 5. 현재 남은 일 (사용자 결정 대기)

1. **GameSir G7 Pro 잔량.** 사용자 데스크톱에서 진단한 결과:
   - 2.4G 동글(VID_3537 PID_100A)이 "Windows용 Xbox 360 컨트롤러"(xusb22, 유선)로 인식된다.
   - 그래서 XInput은 항상 "유선/높음"만 주고, 앱에는 "유선"으로 보인다.
   - 표준 API로는 잔량을 읽을 수 없다. 남은 경로는 GameSir 전용 HID 채널(장치에 명령 보내기)뿐인데, 3번 규칙 위반이라 비추천이다.

   사용자에게 제시한 선택지 (아직 미정):
   ① "유선" 대신 "연결됨 (잔량 알 수 없음)"으로 표시 (추천)
   ② 위젯·트레이에서 빼기 (이미 설정에서 가능)
   ③ 전용 채널 분석 (비추천)
2. **서명 키 삭제 여부.** `~/.config/razer_battery_tray/update_signing_key.pem`은 자동 업데이트를 뺀 뒤로 쓰이지 않는다. 삭제를 권했고 사용자 답을 기다리는 중이다(되돌릴 수 없으니 확인 후에만 삭제).
3. **사용자 데스크톱 PC**는 아직 1.2.1이 아닐 수 있다. 사용자가 직접 교체한다.

## 6. 알아낸 사실·함정 (같은 실수 반복 방지)

- **실제 로그:** CRLF 줄 끝. 이 PC 로그에는 productId 122(Viper), 124(DeathAdder V2 Pro)만 있다.
  - 연결 직후에는 `powerStatus`가 없는 기기 항목이 찍힌다(상태 유지로 처리).
  - `NoCharge_BatteryFull`은 완충이 아니라 "충전 안 함"이다. `off`일 때 level은 믿지 않는다.
- **위젯:** Progman의 **자식 창**으로 넣으면 Windows 11 25H2에서 레이어드 창이 그려지지 않는다.
  - 그래서 Progman을 **소유자**로 둔 최상위 창 + HWND_BOTTOM을 쓴다. Win+D에도 남는다.
- **WinHTTP:** 한 번 불러오면 프로세스에서 내려가지 않는다(메모리 약 1 MB 증가).
  - 그래서 새 버전 확인은 별도 짧은 프로세스(`--update-check`)에서 하고, 결과를 `%LOCALAPPDATA%\RazerBatteryTray\update_check.txt`로 넘긴다.
- **레이어드 창:** 알파 0인 픽셀은 클릭이 통과한다.
  - 배경 불투명도 0%여도 최소 1/255를 남긴다.
  - 호버 중에는 창 전체를 알파 1/255로 채워서 마우스를 놓치지 않게 한다.
- **아이콘 글꼴:** Segoe Fluent Icons(Win11). 없으면 Segoe MDL2 Assets(Win10)로 대체한다.
- **PowerShell 5:**
  - 한글이 든 .ps1은 **UTF-8 BOM**이 있어야 한다.
  - WinRT 목록의 `.Count`가 비어 나오므로 `@(...)`로 감싼다.
  - `FindWindow(class, $null)`은 `[NullString]::Value`를 써야 한다.
- **WSL 셸:** `pgrep -f`/`pkill -f`로 찾으면 자기 셸 명령줄까지 잡혀서 셸이 죽는다. `pgrep -x python3` 후 cmdline을 확인한다.
- **Windows 11 트레이:** 새 아이콘을 숨김 영역에 넣는다.
  - `HKCU\Control Panel\NotifyIconSettings\*\IsPromoted=1`로 보이게 할 수 있다.
  - 단 사용자 설정이므로 바꿨으면 알린다.
- **GameSir 진단 스크립트:** 컨트롤러가 연결된 사용자 데스크톱 PC에서만 의미 있다(노트북에는 없음).

## 7. 작업 방식 (사용자가 좋아한 것)

- 바꾼 뒤에는 테스트, clippy, 실측(메모리·CPU), 스크린샷 등으로 **직접 확인하고**, 확인 못 한 건 솔직히 말한다.
- 답변은 한국어로 간결하게: 무엇을 했는지 → 확인 결과 → 남은 것/결정할 것.
- 사용자 화면을 건드리는 테스트(창 내리기, Win+D 등)는 최소화하고, 했으면 알린다.
