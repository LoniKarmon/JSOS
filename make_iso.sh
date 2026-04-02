#!/bin/bash
# make_iso.sh - Create a dual BIOS/UEFI bootable ISO for JSOS
set -e

# Constants
KERNEL="target/x86_64-os/debug/os"
BUILD_DIR="target/bootloader_build"
BIOS_IMG="$BUILD_DIR/target/x86_64-bootloader/release/boot-bios.img"
UEFI_EFI="$BUILD_DIR/target/x86_64-bootloader/release/BOOTX64.EFI"
ISO_NAME="jsos.iso"
STAGING_DIR="target/iso_staging"

# Check tools
if ! command -v xorriso &> /dev/null || ! command -v mkfs.vfat &> /dev/null || ! command -v mcopy &> /dev/null; then
    echo "Error: Missing tools. Please run:"
    echo "  sudo pacman -S xorriso dosfstools mtools"
    exit 1
fi

# Ensure UEFI binary is built
if [ ! -f "$UEFI_EFI" ]; then
    echo "Building bootloader..."
    sh run_qemu.sh "$KERNEL" --build-only
fi

echo "Creating ISO staging directory..."
rm -rf "$STAGING_DIR"
mkdir -p "$STAGING_DIR/EFI/BOOT"
cp "$UEFI_EFI" "$STAGING_DIR/EFI/BOOT/BOOTX64.EFI"
cp "$KERNEL" "$STAGING_DIR/os"
cp "$BIOS_IMG" "$STAGING_DIR/boot-bios.img"

echo "Creating EFI System Partition image (FAT)..."
ESP_IMG="target/esp.img"
dd if=/dev/zero of="$ESP_IMG" bs=1M count=4 status=none
mkfs.vfat "$ESP_IMG" > /dev/null
mmd -i "$ESP_IMG" ::/EFI
mmd -i "$ESP_IMG" ::/EFI/BOOT
mcopy -i "$ESP_IMG" "$UEFI_EFI" ::/EFI/BOOT/BOOTX64.EFI


echo "Generating Hybrid BIOS/UEFI ISO..."
# 1. Copy your FAT image into the staging area
cp "$ESP_IMG" "$STAGING_DIR/efi.img"

rm -f jsos.iso

xorriso -as mkisofs \
    -iso-level 3 \
    -full-iso9660-filenames \
    -volid "JSOS" \
    -eltorito-boot boot-bios.img \
    -no-emul-boot -boot-load-size 4 -boot-info-table \
    --eltorito-alt-boot \
    -e efi.img \
    -no-emul-boot \
    -isohybrid-gpt-basdat \
    -append_partition 2 0xef "$ESP_IMG" \
    -o "$ISO_NAME" \
    "$STAGING_DIR"

echo "Success! Created $ISO_NAME"
echo "You can now burn this to a DVD or flash it to a USB drive."
