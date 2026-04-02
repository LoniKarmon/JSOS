#!/bin/bash
set -e

TARGET=$1

if [ -z "$TARGET" ]; then
    echo "Usage: $0 <kernel-binary>"
    echo "  e.g. $0 target/x86_64-os/debug/os"
    exit 1
fi

# Build the bootloader (reuse run_qemu.sh's build logic by calling it with --build-only)
sh run_qemu.sh "$TARGET" --build-only

EFI="target/bootloader_build/target/x86_64-bootloader/release/BOOTX64.EFI"
if [ ! -f "$EFI" ]; then
    echo "Error: UEFI binary not found at $EFI"
    exit 1
fi

# Create a FAT16 EFI disk image (FAT32 needs too many clusters for small images)
EFI_IMG="/tmp/efi_boot.img"
dd if=/dev/zero of="$EFI_IMG" bs=1M count=64 status=none
mkfs.fat -F 16 "$EFI_IMG" > /dev/null
mmd -i "$EFI_IMG" ::/EFI ::/EFI/BOOT
mcopy -i "$EFI_IMG" "$EFI" ::/EFI/BOOT/

DISK_IMAGE="jskv.img"
if [ ! -f "$DISK_IMAGE" ]; then
    echo "Creating 64MB persistent storage disk ($DISK_IMAGE)..."
    dd if=/dev/zero of="$DISK_IMAGE" bs=1M count=64 status=none
fi

export GDK_BACKEND=x11

qemu-system-x86_64 \
    -drive "if=pflash,format=raw,readonly=on,file=/usr/share/ovmf/x64/OVMF.4m.fd" \
    -drive "format=raw,file=$EFI_IMG,if=ide,index=0" \
    -drive "format=raw,file=$DISK_IMAGE,if=ide,index=1" \
    -machine pc \
    -netdev "user,id=u1,hostfwd=tcp::8080-:80" \
    -device "rtl8139,netdev=u1" \
    -no-reboot \
    -device "isa-debug-exit,iobase=0xf4,iosize=0x04" \
    -device "qemu-xhci" \
    -device "usb-kbd" \
    -device "usb-mouse" \
    -serial stdio \
    -accel kvm \
    -cpu host \
    -m 1G \
    -vga std \
    -display gtk
