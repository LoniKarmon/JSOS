# Intel I219-LM (e1000e) NIC Driver Design

**Date:** 2026-04-05
**Status:** Approved

## Overview

Add an Intel I219-LM (e1000e family) NIC driver to JSOS alongside the existing RTL8139 driver. Introduce a `NicDriver` trait for the driver interface and a `Nic` enum for smoltcp `Device` trait dispatch. Priority-based init: try e1000e first, fall back to RTL8139. Supports both QEMU emulation (`-device e1000e`) and bare-metal on the HP EliteDesk 800 G6 (Intel i7-10700, 16GB RAM).

## 1. NIC Abstraction Layer (`src/net/nic.rs`)

### NicDriver Trait

Defines the interface every NIC driver must implement:

```rust
pub trait NicDriver {
    fn mac_address(&self) -> [u8; 6];
    fn receive_packet(&mut self) -> Option<Vec<u8>>;
    fn transmit_packet(&mut self, data: &[u8]);
}
```

This is the extension point. Adding a new NIC driver means implementing `NicDriver` on a new struct.

### Nic Enum

Thin dispatch layer that implements smoltcp's `Device` trait. Required because `Device` uses GATs (generic associated types) which prevent `dyn Device`.

```rust
pub enum Nic {
    Rtl8139(Rtl8139),
    E1000e(E1000e),
}
```

The `Device` implementation on `Nic` delegates to the inner type via match arms. Token types wrap the inner driver's tokens.

Adding a new NIC variant requires:
1. Add enum variant
2. Add match arms in `Device` impl
3. Add match arm in `NicDriver` delegation

## 2. E1000e Driver (`src/net/e1000e.rs`)

### Hardware Model

The Intel I219-LM is an e1000e-family Ethernet controller using memory-mapped I/O (MMIO) and descriptor ring-based DMA.

**PCI Identification:**
- Vendor: `0x8086` (Intel)
- Device IDs to scan (I219 variants):
  - `0x0D4C` — I219-LM v10 (Comet Lake, primary target for EliteDesk 800 G6)
  - `0x0D4D` — I219-V v10 (Comet Lake consumer)
  - `0x0D4E` — I219-LM v11 (Comet Lake)
  - `0x0D4F` — I219-V v11 (Comet Lake)
  - `0x15BB` — I219-LM v6
  - `0x15BC` — I219-V v6
  - `0x15BD` — I219-LM v7
  - `0x15BE` — I219-V v7
  - `0x0D53` — I219-LM v12
  - `0x15DF` — I219-LM v8

**Register Access:** MMIO via BAR0. The BAR0 physical address is mapped to a virtual address using the bootloader's physical memory offset (`PHYS_MEM_OFFSET + bar0_phys`). All register reads/writes are volatile 32-bit MMIO operations.

### Key Registers

| Register | Offset | Purpose |
|----------|--------|---------|
| CTRL | 0x0000 | Device control (reset, link up) |
| STATUS | 0x0008 | Device status (link, speed) |
| EERD | 0x0014 | EEPROM read |
| ICR | 0x00C0 | Interrupt cause read |
| IMS | 0x00D0 | Interrupt mask set |
| IMC | 0x00D8 | Interrupt mask clear |
| RCTL | 0x0100 | Receive control |
| TCTL | 0x0400 | Transmit control |
| RDBAL | 0x2800 | RX descriptor base low |
| RDBAH | 0x2804 | RX descriptor base high |
| RDLEN | 0x2808 | RX descriptor ring length (bytes) |
| RDH | 0x2810 | RX descriptor head |
| RDT | 0x2818 | RX descriptor tail |
| TDBAL | 0x3800 | TX descriptor base low |
| TDBAH | 0x3804 | TX descriptor base high |
| TDLEN | 0x3808 | TX descriptor ring length (bytes) |
| TDH | 0x3810 | TX descriptor head |
| TDT | 0x3818 | TX descriptor tail |
| RAL | 0x5400 | Receive address low (MAC bytes 0-3) |
| RAH | 0x5404 | Receive address high (MAC bytes 4-5) |

### Descriptor Rings

**TX Descriptor (legacy format, 16 bytes):**
```
[0:7]   buffer_addr: u64   — physical address of packet buffer
[8:9]   length: u16        — packet length
[10]    cso: u8            — checksum offset
[11]    cmd: u8            — command bits (EOP=0x01, IFCS=0x02, RS=0x08)
[12]    status: u8         — status bits (DD=0x01 = descriptor done)
[13]    css: u8            — checksum start
[14:15] special: u16       — VLAN tag
```

**RX Descriptor (legacy format, 16 bytes):**
```
[0:7]   buffer_addr: u64   — physical address of receive buffer
[8:9]   length: u16        — received packet length (written by hardware)
[10:11] checksum: u16      — packet checksum
[12]    status: u8         — status bits (DD=0x01, EOP=0x02)
[13]    errors: u8         — error bits
[14:15] special: u16       — VLAN tag
```

**Ring Configuration:**
- 256 descriptors per ring (TX and RX)
- Each descriptor: 16 bytes
- Ring size: 256 * 16 = 4096 bytes (one page, naturally aligned)
- Packet buffers: 2048 bytes each
- Total RX buffer memory: 256 * 2048 = 512KB
- Total TX buffer memory: 256 * 2048 = 512KB

**DMA buffers:** Static `#[repr(align(4096))]` arrays, same pattern as RTL8139 but using 64-bit physical addresses via `virt_to_phys64()`.

### Initialization Sequence

1. PCI scan for Intel vendor + I219 device IDs
2. Enable PCI bus mastering
3. Read BAR0 physical address, compute MMIO virtual address
4. Device reset: write `CTRL.RST` (bit 26), wait for clear
5. Disable interrupts: write `0xFFFFFFFF` to IMC
6. Read MAC address from RAL/RAH registers (firmware typically pre-loads these from NVM/EEPROM during platform init). If MAC reads as `00:00:00:00:00:00`, fall back to reading from EEPROM via the EERD register
7. Allocate TX/RX descriptor rings and packet buffers (static arrays)
8. Initialize all RX descriptors with pre-allocated buffer physical addresses
9. Program TX ring: TDBAL/TDBAH = ring phys addr, TDLEN = 4096, TDH = 0, TDT = 0
10. Program RX ring: RDBAL/RDBAH = ring phys addr, RDLEN = 4096, RDH = 0, RDT = 255
11. Configure RCTL: enable, strip CRC, broadcast accept, buffer size 2048
12. Configure TCTL: enable, pad short packets, collision threshold
13. Set link up in CTRL register

### Receive Path

```
poll: read RDH from hardware
      while rx_tail != RDH:
        read descriptor at rx_tail
        if DD bit set:
          copy packet data from buffer (length from descriptor)
          re-initialize descriptor with same buffer address
          advance rx_tail, write to RDT
```

### Transmit Path

```
transmit: read TDH from hardware
          write packet to buffer at tx_tail
          set descriptor: buffer_addr, length, cmd = EOP|IFCS|RS
          advance tx_tail, write to TDT
          (hardware picks up descriptor and sends)
```

### E1000e Struct

```rust
pub struct E1000e {
    mmio_base: u64,         // virtual address of BAR0 MMIO region
    mac_address: [u8; 6],
    rx_head: u16,           // software read pointer
    tx_tail: u16,           // software write pointer
}
```

Descriptor rings and packet buffers are static arrays (not struct fields) — same pattern as RTL8139's `static mut RTL_RX_BUFFER`.

## 3. Memory Changes (`src/memory.rs`)

Add `virt_to_phys64(virt: u64) -> u64` — identical to `virt_to_phys` but returns `u64` without the 32-bit assertion. The existing `virt_to_phys` continues to work for RTL8139.

## 4. Integration Changes

### `src/net/mod.rs`

- Import `nic::{Nic, NicDriver}`
- Change `NIC` global type from `Mutex<Option<Rtl8139>>` to `Mutex<Option<Nic>>`
- `init()` flow:
  1. Call `e1000e::init()` — returns `Option<E1000e>`
  2. If Some, wrap in `Nic::E1000e(nic)`
  3. Else, call `rtl8139::init()` — returns `Option<Rtl8139>`
  4. If Some, wrap in `Nic::Rtl8139(nic)`
  5. Else, no NIC found
  6. Extract MAC via `NicDriver::mac_address()`, configure smoltcp interface
- `poll_network()` unchanged — `iface.poll(nic, sockets)` works because `Nic` implements `Device`

### `src/net/rtl8139.rs`

- Implement `NicDriver` for `Rtl8139`
- Change `init()` to return `Option<Rtl8139>` instead of storing in a global (the global moves to `mod.rs`)
- Remove the `NIC` lazy_static from this file

### `run_qemu.sh`

- Change `-device rtl8139` to `-device e1000e`
- Keep same `user` netdev backend and port forwarding

### `src/pci.rs`

No changes needed. Existing `scan_pci()` already enumerates all devices.

## 5. I219-LM Device ID Coverage

The HP EliteDesk 800 G6 with i7-10700 (Comet Lake-S) most likely has one of:
- `0x0D4C` (I219-LM v10) — most common for Comet Lake vPro
- `0x15BB` (I219-LM v6) — also seen in some G6 models

We scan for all known I219 variants to be safe. The driver is identical for all — they differ only in device ID and minor PHY tuning which doesn't affect basic operation.

## 6. Scope Boundaries

### In Scope
- E1000e driver with MMIO, descriptor rings, polled TX/RX
- NicDriver trait and Nic enum abstraction
- 64-bit DMA support (`virt_to_phys64`)
- Priority-based init (e1000e > RTL8139)
- QEMU switch to `-device e1000e`
- Multiple I219 device ID support

### Out of Scope
- MSI/MSI-X interrupts (polled mode only, matching RTL8139)
- Jumbo frames (standard 1500 MTU)
- Hardware offloading (checksum, TSO)
- Wake-on-LAN
- VLAN tagging
- Multiple NIC support (one NIC active at a time)
- Intel I225/I226 (different driver family)

## 7. Notes

**BSS size increase:** The static descriptor rings and packet buffers add ~1MB to the `.bss` section (256 * 2048 * 2 for RX+TX buffers, plus ring descriptors). This is fine for the 1GB RAM QEMU config and the 16GB EliteDesk, but worth noting if the kernel memory footprint matters.

## 8. Testing Strategy

- **QEMU:** Switch to `-device e1000e`, verify DHCP, DNS, `os.fetch()`, TFTP, FTP all work
- **Bare metal:** Boot on EliteDesk, verify PCI detection, link up, DHCP, basic networking
- **Fallback:** Switch QEMU back to `-device rtl8139`, verify RTL8139 still works as fallback
