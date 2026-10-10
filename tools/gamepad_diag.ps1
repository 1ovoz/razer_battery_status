# 게임패드(GameSir G7 Pro 등) 배터리 진단
# 컨트롤러가 연결된 PC에서 실행:  powershell -ExecutionPolicy Bypass -File tools\gamepad_diag.ps1
# 결과는 화면에 나오고, 같은 폴더의 gamepad_diag.txt에도 저장된다.
# 읽기만 한다: 장치 목록 조회, XInput 배터리 조회, Windows.Gaming.Input 배터리 조회.

$out = Join-Path $PSScriptRoot 'gamepad_diag.txt'
$lines = New-Object System.Collections.Generic.List[string]
function Say($s) { Write-Host $s; $lines.Add($s) }

Say "== 1. 컨트롤러로 보이는 장치 (USB/HID)"
# Win32_PnPEntity 한 번 조회로 이름·드라이버(Service)를 같이 가져온다 (장치별 조회보다 훨씬 빠름)
Get-CimInstance Win32_PnPEntity | Where-Object {
    $_.PNPDeviceID -match '^(USB|HID|BTHLE)' -and (
        $_.PNPDeviceID -match 'VID_3537|VID_045E' -or
        $_.Name -match 'GameSir|Xbox|Gamepad|Game ?Controller|게임 컨트롤러|게임 패드|게임패드|Gaming|XINPUT' -or
        $_.Service -in @('xusb22', 'xboxgip', 'dc1-controller'))
} | ForEach-Object { Say ("  {0} | {1} | svc={2} | {3}" -f $_.PNPClass, $_.Name, $_.Service, $_.PNPDeviceID) }

Say ""
Say "== 2. XInput (지금 앱이 쓰는 방식)"
Add-Type -TypeDefinition @'
using System; using System.Runtime.InteropServices;
public static class XI {
  [StructLayout(LayoutKind.Sequential)] public struct BAT { public byte Type; public byte Level; }
  [DllImport("xinput1_4.dll")] public static extern uint XInputGetBatteryInformation(uint user, byte devType, out BAT info);
}
'@
$types = @{ 0 = '연결 안 됨'; 1 = '유선(WIRED)'; 2 = '건전지'; 3 = '충전지(NIMH)'; 255 = '알 수 없음' }
$levels = @{ 0 = '거의 없음'; 1 = '낮음'; 2 = '보통'; 3 = '높음' }
foreach ($u in 0..3) {
    $b = New-Object XI+BAT
    $r = [XI]::XInputGetBatteryInformation($u, 0, [ref]$b)
    if ($r -eq 0) { Say ("  슬롯 {0}: 종류={1} 단계={2}" -f $u, $types[[int]$b.Type], $levels[[int]$b.Level]) }
    else { Say ("  슬롯 {0}: 없음" -f $u) }
}

Say ""
Say "== 3. Windows.Gaming.Input (다른 방식 후보)"
try {
    [void][Windows.Gaming.Input.RawGameController, Windows.Gaming.Input, ContentType = WindowsRuntime]
    Start-Sleep -Seconds 2   # 컨트롤러 목록이 비동기로 채워지는 시간
    # PowerShell 5에서는 WinRT 목록의 .Count가 비어 나오므로 배열로 바꿔서 센다
    $list = @([Windows.Gaming.Input.RawGameController]::RawGameControllers)
    if ($list.Count -eq 0) { Say "  컨트롤러 없음" }
    foreach ($c in $list) {
        Say ("  {0} | VID={1:X4} PID={2:X4} | 무선={3}" -f $c.DisplayName, $c.HardwareVendorId, $c.HardwareProductId, $c.IsWireless)
        $rep = $c.TryGetBatteryReport()
        if ($null -eq $rep) { Say "    배터리 보고: 없음" }
        else {
            Say ("    배터리 상태={0} 남은용량={1}mWh 완충용량={2}mWh 충전속도={3}mW" -f $rep.Status, $rep.RemainingCapacityInMilliwattHours, $rep.FullChargeCapacityInMilliwattHours, $rep.ChargeRateInMilliwatts)
        }
    }
} catch { Say "  조회 실패: $($_.Exception.Message)" }

Say ""
Say "== 4. GameSir 앱"
$apps = Get-ChildItem HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall, HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall, HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall -ErrorAction SilentlyContinue |
    ForEach-Object { (Get-ItemProperty $_.PSPath).DisplayName } | Where-Object { $_ -match 'gamesir|nexus' }
$store = Get-AppxPackage -ErrorAction SilentlyContinue | Where-Object { $_.Name -match 'gamesir|nexus' } | ForEach-Object { $_.Name }
if (-not $apps -and -not $store) { Say "  설치된 GameSir 앱 없음" } else { $apps + $store | ForEach-Object { Say "  $_" } }

$lines | Set-Content -Path $out -Encoding UTF8
Write-Host ""
Write-Host "저장됨: $out"
