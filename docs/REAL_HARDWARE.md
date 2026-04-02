# JSOS Real Hardware Boot Guide

This guide explains how to take the JSOS build and boot it on a physical x86_64 computer.

## ⚠️ Warning
**Flash at your own risk.** Using the `dd` command or other disk-flashing tools can erase your data if you select the wrong drive. Always double-check your target device path.

---

## 🏗 Preparation

First, ensure you have built the OS and bootloader:
```bash
# This builds both BIOS and UEFI versions
sh run_qemu.sh target/x86_64-os/debug/os
```

The build process generates:
- **BIOS Image**: `target/bootloader_build/target/x86_64-bootloader/release/boot-bios.img` (Raw disk image)
- **UEFI Binary**: `target/bootloader_build/target/x86_64-bootloader/release/BOOTX64.EFI` (EFI executable)

---

## 💾 Option 1: Legacy BIOS Boot

Most older PCs (and modern ones with "CSM" or "Legacy" mode enabled) can boot the raw BIOS image directly from a USB drive.

**Using Linux/macOS (`dd`):**
1. Identify your USB drive (e.g., `/dev/sdX` or `/dev/diskX`).
2. Run the following command:
   ```bash
   sudo dd if=target/bootloader_build/target/x86_64-bootloader/release/boot-bios.img of=/dev/sdX bs=4M status=progress
   ```
3. Boot the PC and select the USB drive in the BIOS menu.

**Using Windows:**
- Use a tool like **Rufus** or **BalenaEtcher**.
- Select the `boot-bios.img` as the source and your USB drive as the target.

---

## ⚡ Option 2: UEFI Boot (Modern PCs)

UEFI is the standard for modern hardware. It requires a FAT32-formatted USB drive.

1. **Format a USB drive** to FAT32 (MBR or GPT partition table).
2. **Create the directory structure**:
   ```bash
   mkdir -p /path/to/usb/EFI/BOOT/
   ```
3. **Copy the bootloader**:
   Copy `target/bootloader_build/target/x86_64-bootloader/release/BOOTX64.EFI` to the USB at `/EFI/BOOT/BOOTX64.EFI`.
4. **Copy the kernel**:
   Copy your kernel binary (`target/x86_64-os/debug/os`) to the root of the USB drive.
5. **Boot**: Disable "Secure Boot" in your PC's UEFI settings and select the USB drive.

---

## 📡 Networking & Input
- **Keyboard/Mouse**: Standard USB keyboards and mice work via the kernel's PS/2 emulation or legacy support (ensure "Legacy USB Support" is enabled in BIOS).
- **Network**: The current driver specifically supports **Realtek RTL8139** NICs. Other cards will not be detected.
- **Display**: High-resolution support depends on VESA/GOP compatibility of your graphics card.

