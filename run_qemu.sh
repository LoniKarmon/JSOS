#!/bin/bash
set -e

# Kill lingering QEMU instances
echo "Killing any existing QEMU processes..."
pkill -f qemu-system-x86_64 || true
sleep 1 # Wait for locks to be released

# Configuration
TARGET=$1
BOOTLOADER_VERSION="0.10.13"
MANIFEST="Cargo.toml"

if [ -z "$TARGET" ]; then
    echo "Usage: $0 <kernel-binary> [--build-only]"
    exit 1
fi

BUILD_ONLY=false
if [ "$2" == "--build-only" ] || [ "$1" == "--build-only" ]; then
    BUILD_ONLY=true
    if [ "$1" == "--build-only" ]; then
        TARGET=$2
    fi
fi

# Environment variables for build-std
export RUSTFLAGS="-Z unstable-options"
export RUSTC_BOOTSTRAP=1
export CARGO_UNSTABLE_BUILD_STD="core,compiler_builtins,alloc"
export CARGO_UNSTABLE_JSON_TARGET_SPEC="true"

# Find bootloader source using cargo metadata
BOOTLOADER_DIR=$(cargo metadata --format-version 1 | jq -r ".packages[] | select(.name == \"bootloader\" and .version == \"$BOOTLOADER_VERSION\") | .manifest_path" | xargs dirname)

if [ -z "$BOOTLOADER_DIR" ] || [ "$BOOTLOADER_DIR" == "." ]; then
    # Fallback if jq is not installed or other failure
    BOOTLOADER_DIR=$(find ~/.cargo/registry/src -name "bootloader-$BOOTLOADER_VERSION" -type d | head -n 1)
fi

if [ -z "$BOOTLOADER_DIR" ]; then
    echo "Error: Could not find bootloader-$BOOTLOADER_VERSION"
    echo "Try running 'cargo fetch' first."
    exit 1
fi

export KERNEL=$(realpath "$TARGET")
export KERNEL_MANIFEST=$(realpath "$MANIFEST")

BUILD_DIR="target/bootloader_build"
BOOTLOADER_IMG="$BUILD_DIR/target/x86_64-bootloader/release/boot-bios.img"
BOOTLOADER_UEFI="$BUILD_DIR/target/x86_64-bootloader/release/boot-uefi.img"

# Initialize bootloader build directory if needed
if [ ! -f "$BUILD_DIR/Cargo.toml" ]; then
    echo "Initializing bootloader build directory..."
    mkdir -p "$BUILD_DIR"
    cp -r "$BOOTLOADER_DIR"/* "$BUILD_DIR/"
    chmod -R +w "$BUILD_DIR"
    echo -e "\n\n[workspace]\n" >> "$BUILD_DIR/Cargo.toml"
fi

# Optimize: skip build if image is newer than kernel and manifest, and UEFI binary exists
BOOT_UEFI="$BUILD_DIR/target/x86_64-bootloader/release/BOOTX64.EFI"
if [ -f "$BOOTLOADER_IMG" ] && [ -f "$BOOT_UEFI" ] && [ "$BOOTLOADER_IMG" -nt "$TARGET" ] && [ "$BOOTLOADER_IMG" -nt "$MANIFEST" ]; then
    echo "Bootloader image is up to date, skipping build"
else
    pushd "$BUILD_DIR" > /dev/null
    
    # Update lockfile if needed (matching ps1 logic)
    if [ ! -f "Cargo.lock" ]; then
        rustup run nightly-2023-10-31 cargo update -Z next-lockfile-bump -p x86_64 --precise 0.14.11
    fi

    # Build bootloader (BIOS)
    rustup run nightly-2023-10-31 cargo build --bin bios --release \
        -Z unstable-options -Z next-lockfile-bump \
        --target "../../x86_64-bootloader.json" \
        --features bios_bin \
        -Zbuild-std=core -Zbuild-std-features=compiler-builtins-mem
    
    # Build bootloader (UEFI)
    rustup run nightly-2023-10-31 cargo build --bin uefi --release \
        -Z unstable-options -Z next-lockfile-bump \
        --target x86_64-unknown-uefi \
        --features uefi_bin \
        -Zbuild-std=core -Zbuild-std-features=compiler-builtins-mem
    
    popd > /dev/null

    # Convert ELF to flat binary
    BOOTLOADER_ELF="$BUILD_DIR/target/x86_64-bootloader/release/bios"
    SYSROOT=$(rustup run nightly-2023-10-31 rustc --print sysroot)
    OBJCOPY="$SYSROOT/lib/rustlib/x86_64-unknown-linux-gnu/bin/llvm-objcopy"
    
    if [ ! -f "$OBJCOPY" ]; then
        # Fallback to system objcopy if rust toolchain one isn't found
        OBJCOPY="objcopy"
    fi

    $OBJCOPY -I elf64-x86-64 -O binary --strip-debug "$BOOTLOADER_ELF" "$BOOTLOADER_IMG"

    # Create UEFI image (requires mtools or similar, fallback to just copying the .efi if not available)
    BOOTLOADER_EFI="$BUILD_DIR/target/x86_64-unknown-uefi/release/uefi.efi"
    if [ ! -f "$BOOTLOADER_EFI" ]; then
        BOOTLOADER_EFI="$BUILD_DIR/target/x86_64-unknown-uefi/release/uefi"
    fi
    cp "$BOOTLOADER_EFI" "$BUILD_DIR/target/x86_64-bootloader/release/BOOTX64.EFI"
    echo "UEFI binary built at $BOOTLOADER_EFI"
fi

DISK_IMAGE="jskv.img"
if [ ! -f "$DISK_IMAGE" ]; then
    echo "Creating 64MB persistent storage disk ($DISK_IMAGE)..."
    dd if=/dev/zero of=$DISK_IMAGE bs=1M count=64 status=none
fi

# Skip QEMU if build-only
if [ "$BUILD_ONLY" = true ]; then
    echo "Build complete. Skipping QEMU launch."
    exit 0
fi

# Run QEMU
QEMU="qemu-system-x86_64"
QEMU_ARGS=(
    "-drive" "format=raw,file=$BOOTLOADER_IMG"
    "-drive" "format=raw,file=$DISK_IMAGE,media=disk,index=1"
    "-machine" "pc"
    "-netdev" "user,id=u1,hostfwd=tcp::8080-:80"
    "-device" "e1000e,netdev=u1"
    "-no-reboot"
    "-device" "isa-debug-exit,iobase=0xf4,iosize=0x04"
    "-device" "qemu-xhci"
    "-device" "usb-kbd"
    "-device" "usb-mouse"
    "-serial" "stdio"
    "-accel" "kvm"
    "-cpu" "host"
    "-m" "1G"
    "-vga" "std"
    "-display" "gtk"
)

export GDK_BACKEND=x11

$QEMU "${QEMU_ARGS[@]}"
EXIT_CODE=$?

if [ $EXIT_CODE -eq 33 ]; then
    exit 0
elif [ $EXIT_CODE -eq 35 ]; then
    exit 1
else
    exit $EXIT_CODE
fi
