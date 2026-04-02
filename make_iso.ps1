$ErrorActionPreference = "Stop"

$CurrentDir = (Get-Location).Path

# Constants
$Kernel      = "target\x86_64-os\debug\os"
$BuildDir    = "target\bootloader_build"
$BiosImg     = "$BuildDir\target\x86_64-bootloader\release\boot-bios.img"
$UefiEfi     = "$BuildDir\target\x86_64-bootloader\release\BOOTX64.EFI"
$IsoName     = "jsos.iso"
$StagingDir  = "target\iso_staging"
$EspImg      = "target\esp.img"

# Require WSL for ISO tooling (xorriso, mkfs.vfat, mcopy)
if (-Not (Get-Command wsl -ErrorAction SilentlyContinue)) {
    Write-Error "WSL is required for ISO creation (xorriso, mkfs.vfat, mcopy). Enable WSL and install: sudo apt install xorriso dosfstools mtools"
    exit 1
}

$MissingTools = wsl bash -c "command -v xorriso && command -v mkfs.vfat && command -v mcopy" 2>&1
if ($LASTEXITCODE -ne 0) {
    Write-Error "Missing tools in WSL. Run: sudo apt install xorriso dosfstools mtools"
    exit 1
}

# Ensure bootloader is built
if (-Not (Test-Path $UefiEfi)) {
    Write-Host "Building bootloader..."
    & ".\run_qemu.ps1" -Target $Kernel -BuildOnly
    if ($LASTEXITCODE -ne 0) { exit 1 }
}

Write-Host "Creating ISO staging directory..."
if (Test-Path $StagingDir) { Remove-Item -Recurse -Force $StagingDir }
New-Item -ItemType Directory -Path "$StagingDir\EFI\BOOT" | Out-Null
Copy-Item $UefiEfi  "$StagingDir\EFI\BOOT\BOOTX64.EFI"
Copy-Item $Kernel   "$StagingDir\os"
Copy-Item $BiosImg  "$StagingDir\boot-bios.img"

Write-Host "Creating EFI System Partition image (FAT)..."
# Use WSL for the FAT image tools
$WslCurrentDir = wsl wslpath -u $CurrentDir.Replace('\', '/')
wsl bash -c @"
set -e
cd '$WslCurrentDir'
dd if=/dev/zero of=target/esp.img bs=1M count=4 status=none
mkfs.vfat target/esp.img > /dev/null
mmd -i target/esp.img ::/EFI
mmd -i target/esp.img ::/EFI/BOOT
mcopy -i target/esp.img '$($UefiEfi.Replace('\','/'))' ::/EFI/BOOT/BOOTX64.EFI
"@
if ($LASTEXITCODE -ne 0) { Write-Error "ESP image creation failed"; exit 1 }

Copy-Item $EspImg "$StagingDir\efi.img"

if (Test-Path $IsoName) { Remove-Item $IsoName }

Write-Host "Generating Hybrid BIOS/UEFI ISO..."
$WslStagingDir = wsl wslpath -u "$CurrentDir\$StagingDir".Replace('\', '/')
$WslEspImg     = wsl wslpath -u "$CurrentDir\$EspImg".Replace('\', '/')
wsl xorriso -as mkisofs `
    -iso-level 3 `
    -full-iso9660-filenames `
    -volid "JSOS" `
    -eltorito-boot boot-bios.img `
    -no-emul-boot -boot-load-size 4 -boot-info-table `
    --eltorito-alt-boot `
    -e efi.img `
    -no-emul-boot `
    -isohybrid-gpt-basdat `
    -append_partition 2 0xef $WslEspImg `
    -o $IsoName `
    $WslStagingDir
if ($LASTEXITCODE -ne 0) { Write-Error "xorriso failed"; exit 1 }

Write-Host "Success! Created $IsoName"
Write-Host "You can now burn this to a DVD or flash it to a USB drive."
