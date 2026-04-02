param (
    [string]$Target,
    [switch]$BuildOnly
)

$ErrorActionPreference = "Stop"

# Kill lingering QEMU instances
Write-Host "Killing any existing QEMU processes..."
Get-Process -Name "qemu-system-x86_64" -ErrorAction SilentlyContinue | Stop-Process -Force

if (-Not $Target) {
    Write-Error "Usage: .\run_qemu.ps1 <kernel-binary> [-BuildOnly]"
    exit 1
}

$Manifest = Resolve-Path "Cargo.toml" | Select-Object -ExpandProperty Path
$Target   = Resolve-Path $Target      | Select-Object -ExpandProperty Path

$env:RUSTFLAGS                       = "-Z unstable-options"
$env:RUSTC_BOOTSTRAP                 = "1"
$env:CARGO_UNSTABLE_BUILD_STD        = "core,compiler_builtins,alloc"
$env:CARGO_UNSTABLE_JSON_TARGET_SPEC = "true"

$CurrentDir = (Get-Location).Path
$env:RUST_TARGET_PATH = "$CurrentDir;$env:RUST_TARGET_PATH"

$BootloaderDir = Get-ChildItem -Path "$HOME\.cargo\registry\src\index.crates.io-*\bootloader-0.10.13" -Directory `
    | Select-Object -First 1 -ExpandProperty FullName
if (-Not $BootloaderDir) {
    Write-Error "Could not find bootloader-0.10.13 in cargo registry"
    exit 1
}

$env:KERNEL          = $Target
$env:KERNEL_MANIFEST = $Manifest

$BuildDir      = Join-Path $CurrentDir "target\bootloader_build"
$CargoTomlPath = Join-Path $BuildDir "Cargo.toml"

if (-Not (Test-Path $CargoTomlPath)) {
    Write-Host "Initializing bootloader build directory..."
    if (Test-Path $BuildDir) { Remove-Item -Recurse -Force $BuildDir }
    New-Item -ItemType Directory -Path $BuildDir | Out-Null
    Copy-Item -Path "$BootloaderDir\*" -Destination $BuildDir -Recurse -Force
    Get-ChildItem -Path $BuildDir -Recurse -File | ForEach-Object { $_.IsReadOnly = $false }
    Add-Content -Path $CargoTomlPath -Value "`n`n[workspace]`n"
}

$BootloaderBin  = Join-Path $BuildDir "target\x86_64-bootloader\release\boot-bios.img"
$BootloaderUefi = Join-Path $BuildDir "target\x86_64-bootloader\release\BOOTX64.EFI"

$NeedsBuild = $true
if ((Test-Path $BootloaderBin) -and (Test-Path $BootloaderUefi)) {
    $KernelTime = (Get-Item $Target).LastWriteTime
    $ManifestTime = (Get-Item $Manifest).LastWriteTime
    $ImageTime  = (Get-Item $BootloaderBin).LastWriteTime
    if ($ImageTime -gt $KernelTime -and $ImageTime -gt $ManifestTime) {
        Write-Host "Bootloader image is up to date, skipping build"
        $NeedsBuild = $false
    }
}

if ($NeedsBuild) {
    Push-Location $BuildDir

    if (-Not (Test-Path "Cargo.lock")) {
        rustup run nightly-2023-10-31 cargo update -Z next-lockfile-bump -p x86_64 --precise 0.14.11
    }

    Remove-Item Env:\RUSTC         -ErrorAction SilentlyContinue
    Remove-Item Env:\RUSTUP_TOOLCHAIN -ErrorAction SilentlyContinue
    Remove-Item Env:\CARGO         -ErrorAction SilentlyContinue

    # Build BIOS bootloader
    rustup run nightly-2023-10-31 cargo build --bin bios --release `
        -Z unstable-options -Z next-lockfile-bump `
        --target "$CurrentDir\x86_64-bootloader.json" `
        --features bios_bin `
        -Zbuild-std=core -Zbuild-std-features=compiler-builtins-mem
    if ($LASTEXITCODE -ne 0) { Pop-Location; Write-Error "BIOS bootloader build failed"; exit 1 }

    # Build UEFI bootloader
    rustup run nightly-2023-10-31 cargo build --bin uefi --release `
        -Z unstable-options -Z next-lockfile-bump `
        --target x86_64-unknown-uefi `
        --features uefi_bin `
        -Zbuild-std=core -Zbuild-std-features=compiler-builtins-mem
    if ($LASTEXITCODE -ne 0) { Pop-Location; Write-Error "UEFI bootloader build failed"; exit 1 }

    Pop-Location

    # Convert ELF to flat binary
    $BootloaderElf = Join-Path $BuildDir "target\x86_64-bootloader\release\bios"
    $Sysroot       = (rustup run nightly-2023-10-31 rustc --print sysroot).Trim()
    $LlvmObjcopy   = Join-Path $Sysroot "lib\rustlib\x86_64-pc-windows-msvc\bin\llvm-objcopy.exe"
    if (-Not (Test-Path $LlvmObjcopy)) {
        Write-Error "llvm-objcopy not found at $LlvmObjcopy. Run: rustup component add llvm-tools-preview --toolchain nightly-2023-10-31"
        exit 1
    }
    & $LlvmObjcopy -I elf64-x86-64 -O binary --strip-debug $BootloaderElf $BootloaderBin

    # Copy UEFI binary
    $UefiSrc = Join-Path $BuildDir "target\x86_64-unknown-uefi\release\uefi.efi"
    if (-Not (Test-Path $UefiSrc)) {
        $UefiSrc = Join-Path $BuildDir "target\x86_64-unknown-uefi\release\uefi"
    }
    Copy-Item $UefiSrc $BootloaderUefi
    Write-Host "UEFI binary built at $UefiSrc"
}

if ($BuildOnly) {
    Write-Host "Build complete. Skipping QEMU launch."
    exit 0
}

$DiskImage = "jskv.img"
if (-Not (Test-Path $DiskImage)) {
    Write-Host "Creating 64MB persistent storage disk ($DiskImage)..."
    fsutil file createnew $DiskImage 67108864 | Out-Null
}

$Qemu = "qemu-system-x86_64"
if (-Not (Get-Command $Qemu -ErrorAction SilentlyContinue)) {
    foreach ($Path in @(
        "C:\Program Files\qemu\qemu-system-x86_64.exe",
        "D:\Programs\QEMU\qemu-system-x86_64.exe"
    )) {
        if (Test-Path $Path) { $Qemu = $Path; break }
    }
}

$QemuArgs = @(
    "-drive",  "format=raw,file=$BootloaderBin",
    "-drive",  "format=raw,file=$DiskImage,media=disk,index=1",
    "-machine","pc",
    "-netdev", "user,id=u1,hostfwd=tcp::8080-:80",
    "-device", "rtl8139,netdev=u1",
    "-no-reboot",
    "-device", "isa-debug-exit,iobase=0xf4,iosize=0x04",
    "-device", "qemu-xhci",
    "-device", "usb-kbd",
    "-device", "usb-mouse",
    "-serial", "stdio",
    "-m",      "1G",
    "-vga",    "std"
)

& $Qemu $QemuArgs
$QemuExitCode = $LASTEXITCODE

if ($QemuExitCode -eq 33) {
    exit 0
} elseif ($QemuExitCode -eq 35) {
    exit 1
} else {
    exit $QemuExitCode
}
