use crate::pci::{PciDevice, scan_pci, enable_bus_mastering};
use crate::memory::{virt_to_phys64, PHYS_MEM_OFFSET};
use crate::serial_println;
use alloc::vec::Vec;
use alloc::vec;
use core::sync::atomic::Ordering;

// PCI IDs
const INTEL_VENDOR: u16 = 0x8086;
const I219_DEVICE_IDS: &[u16] = &[
    0x0D4C, 0x0D4D, 0x0D4E, 0x0D4F, // Comet Lake I219 variants
    0x15BB, 0x15BC, 0x15BD, 0x15BE, // I219 v6/v7
    0x0D53, 0x15DF,                  // I219 v8/v12
    0x10D3,                          // Intel 82574L (QEMU e1000e emulation)
];

// Register offsets (u32 from MMIO base)
const REG_CTRL:  u32 = 0x0000;
const REG_STATUS: u32 = 0x0008;
const REG_EERD:  u32 = 0x0014;
const REG_ICR:   u32 = 0x00C0;
const REG_IMS:   u32 = 0x00D0;
const REG_IMC:   u32 = 0x00D8;
const REG_RCTL:  u32 = 0x0100;
const REG_TCTL:  u32 = 0x0400;
const REG_RDBAL: u32 = 0x2800;
const REG_RDBAH: u32 = 0x2804;
const REG_RDLEN: u32 = 0x2808;
const REG_RDH:   u32 = 0x2810;
const REG_RDT:   u32 = 0x2818;
const REG_TDBAL: u32 = 0x3800;
const REG_TDBAH: u32 = 0x3804;
const REG_TDLEN: u32 = 0x3808;
const REG_TDH:   u32 = 0x3810;
const REG_TDT:   u32 = 0x3818;
const REG_RAL:   u32 = 0x5400;
const REG_RAH:   u32 = 0x5404;

// CTRL bits
const CTRL_RST:  u32 = 1 << 26;
const CTRL_SLU:  u32 = 1 << 6;
const CTRL_ASDE: u32 = 1 << 5;

// RCTL bits
const RCTL_EN:       u32 = 1 << 1;
const RCTL_BAM:      u32 = 1 << 15;
const RCTL_SECRC:    u32 = 1 << 26;
// BSIZE_2048 = 0 (default)

// TCTL bits
const TCTL_EN:        u32 = 1 << 1;
const TCTL_PSP:       u32 = 1 << 3;
const TCTL_CT_SHIFT:  u32 = 4;
const TCTL_COLD_SHIFT: u32 = 12;

// TX command bits
const TX_CMD_EOP:  u8 = 1 << 0;
const TX_CMD_IFCS: u8 = 1 << 1;
const TX_CMD_RS:   u8 = 1 << 3;

// Descriptor status bits
const STATUS_DD:     u8 = 1 << 0;
const STATUS_RX_EOP: u8 = 1 << 1;

// Ring/buffer sizes
const NUM_RX_DESC: usize = 256;
const NUM_TX_DESC: usize = 256;
const PACKET_BUF_SIZE: usize = 2048;

// Each legacy descriptor is exactly 16 bytes
const DESC_SIZE: usize = 16;

/// RX descriptor (legacy format) — 16 bytes
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct RxDescriptor {
    pub buffer_addr: u64,
    pub length:      u16,
    pub checksum:    u16,
    pub status:      u8,
    pub errors:      u8,
    pub special:     u16,
}

/// TX descriptor (legacy format) — 16 bytes
#[repr(C, packed)]
#[derive(Copy, Clone)]
pub struct TxDescriptor {
    pub buffer_addr: u64,
    pub length:      u16,
    pub cso:         u8,
    pub cmd:         u8,
    pub status:      u8,
    pub css:         u8,
    pub special:     u16,
}

// ---------------------------------------------------------------------------
// Static DMA buffers — page-aligned so the NIC can map them directly
// ---------------------------------------------------------------------------

#[repr(align(4096))]
struct DescRingWrapper<const N: usize>([u8; N]);

#[repr(align(4096))]
struct PacketBufferPool<const N: usize>([u8; N]);

static mut RX_DESC_RING: DescRingWrapper<{ NUM_RX_DESC * DESC_SIZE }> =
    DescRingWrapper([0u8; NUM_RX_DESC * DESC_SIZE]);

static mut TX_DESC_RING: DescRingWrapper<{ NUM_TX_DESC * DESC_SIZE }> =
    DescRingWrapper([0u8; NUM_TX_DESC * DESC_SIZE]);

static mut RX_PACKET_BUFS: PacketBufferPool<{ NUM_RX_DESC * PACKET_BUF_SIZE }> =
    PacketBufferPool([0u8; NUM_RX_DESC * PACKET_BUF_SIZE]);

static mut TX_PACKET_BUFS: PacketBufferPool<{ NUM_TX_DESC * PACKET_BUF_SIZE }> =
    PacketBufferPool([0u8; NUM_TX_DESC * PACKET_BUF_SIZE]);

// ---------------------------------------------------------------------------
// Driver struct
// ---------------------------------------------------------------------------

pub struct E1000e {
    mmio_base:   u64,
    mac_address: [u8; 6],
    rx_tail:     u16,
    tx_tail:     u16,
}

// ---------------------------------------------------------------------------
// MMIO helpers
// ---------------------------------------------------------------------------

impl E1000e {
    #[inline]
    fn read_reg(&self, offset: u32) -> u32 {
        unsafe {
            core::ptr::read_volatile((self.mmio_base + offset as u64) as *const u32)
        }
    }

    #[inline]
    fn write_reg(&self, offset: u32, value: u32) {
        unsafe {
            core::ptr::write_volatile((self.mmio_base + offset as u64) as *mut u32, value);
        }
    }
}

// ---------------------------------------------------------------------------
// Initialisation
// ---------------------------------------------------------------------------

impl E1000e {
    pub fn new(device: &PciDevice) -> Option<Self> {
        enable_bus_mastering(device);

        // BAR0 holds the 32-bit physical base of the MMIO region.
        let bar0_phys = device.get_bar_memory_address(0) as u64;
        if bar0_phys == 0 {
            serial_println!("e1000e: BAR0 is zero — aborting");
            return None;
        }

        let phys_offset = PHYS_MEM_OFFSET.load(Ordering::Relaxed);
        let mmio_base = phys_offset + bar0_phys;

        let mut nic = E1000e {
            mmio_base,
            mac_address: [0u8; 6],
            rx_tail: 0,
            tx_tail: 0,
        };

        // 1. Reset
        nic.reset();

        // 2. Disable all interrupts
        nic.write_reg(REG_IMC, 0xFFFF_FFFF);
        let _ = nic.read_reg(REG_ICR); // flush pending

        // 3. Read MAC address
        nic.mac_address = nic.read_mac();
        let m = nic.mac_address;
        serial_println!(
            "e1000e: MAC {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
            m[0], m[1], m[2], m[3], m[4], m[5]
        );

        // 4. Set up descriptor rings
        nic.init_rx();
        nic.init_tx();

        // 5. Enable RX / TX
        nic.write_reg(REG_RCTL, RCTL_EN | RCTL_BAM | RCTL_SECRC);
        nic.write_reg(
            REG_TCTL,
            TCTL_EN | TCTL_PSP | (15 << TCTL_CT_SHIFT) | (64 << TCTL_COLD_SHIFT),
        );

        // 6. Force link up
        let ctrl = nic.read_reg(REG_CTRL);
        nic.write_reg(REG_CTRL, ctrl | CTRL_SLU | CTRL_ASDE);

        serial_println!("e1000e: init complete");
        Some(nic)
    }

    // -----------------------------------------------------------------------
    // Reset
    // -----------------------------------------------------------------------

    fn reset(&self) {
        let ctrl = self.read_reg(REG_CTRL);
        self.write_reg(REG_CTRL, ctrl | CTRL_RST);
        // Spin until the RST bit self-clears
        let mut spins = 0u32;
        loop {
            core::hint::spin_loop();
            if self.read_reg(REG_CTRL) & CTRL_RST == 0 {
                break;
            }
            spins += 1;
            if spins > 1_000_000 {
                serial_println!("e1000e: reset timeout!");
                break;
            }
        }
    }

    // -----------------------------------------------------------------------
    // MAC address
    // -----------------------------------------------------------------------

    fn read_mac(&self) -> [u8; 6] {
        let ral = self.read_reg(REG_RAL);
        let rah = self.read_reg(REG_RAH);

        if ral != 0 || (rah & 0xFFFF) != 0 {
            // RAH/RAL are already programmed
            return [
                (ral & 0xFF) as u8,
                ((ral >> 8) & 0xFF) as u8,
                ((ral >> 16) & 0xFF) as u8,
                ((ral >> 24) & 0xFF) as u8,
                (rah & 0xFF) as u8,
                ((rah >> 8) & 0xFF) as u8,
            ];
        }

        // Fall back to EEPROM
        serial_println!("e1000e: RAL/RAH are zero, reading MAC from EEPROM");
        self.read_mac_eeprom()
    }

    fn read_mac_eeprom(&self) -> [u8; 6] {
        let mut mac = [0u8; 6];
        for i in 0..3u32 {
            // Write address + start bit
            self.write_reg(REG_EERD, (i << 8) | 1);
            // Poll done bit (bit 4)
            for _ in 0..10_000 {
                let val = self.read_reg(REG_EERD);
                if val & (1 << 4) != 0 {
                    let data = (val >> 16) as u16;
                    mac[(i * 2) as usize]     = (data & 0xFF) as u8;
                    mac[(i * 2 + 1) as usize] = (data >> 8) as u8;
                    break;
                }
                core::hint::spin_loop();
            }
        }
        mac
    }

    // -----------------------------------------------------------------------
    // Descriptor ring setup
    // -----------------------------------------------------------------------

    fn init_rx(&mut self) {
        unsafe {
            // Obtain raw pointer to the static ring byte array
            let ring_ptr = RX_DESC_RING.0.as_mut_ptr() as *mut RxDescriptor;
            let bufs_ptr = RX_PACKET_BUFS.0.as_mut_ptr();

            for i in 0..NUM_RX_DESC {
                let desc = &mut *ring_ptr.add(i);
                let buf_virt = bufs_ptr.add(i * PACKET_BUF_SIZE) as u64;
                desc.buffer_addr = virt_to_phys64(buf_virt);
                desc.length   = 0;
                desc.checksum = 0;
                desc.status   = 0;
                desc.errors   = 0;
                desc.special  = 0;
            }

            let ring_phys = virt_to_phys64(RX_DESC_RING.0.as_ptr() as u64);
            self.write_reg(REG_RDBAL, (ring_phys & 0xFFFF_FFFF) as u32);
            self.write_reg(REG_RDBAH, (ring_phys >> 32) as u32);
            self.write_reg(REG_RDLEN, (NUM_RX_DESC * DESC_SIZE) as u32);
            self.write_reg(REG_RDH, 0);
            // Set tail to last descriptor so the NIC owns all descriptors
            self.write_reg(REG_RDT, (NUM_RX_DESC - 1) as u32);
            // We start polling from descriptor 0
            self.rx_tail = 0;
        }
    }

    fn init_tx(&mut self) {
        unsafe {
            let ring_ptr = TX_DESC_RING.0.as_mut_ptr() as *mut TxDescriptor;
            let bufs_ptr = TX_PACKET_BUFS.0.as_mut_ptr();

            for i in 0..NUM_TX_DESC {
                let desc = &mut *ring_ptr.add(i);
                let buf_virt = bufs_ptr.add(i * PACKET_BUF_SIZE) as u64;
                desc.buffer_addr = virt_to_phys64(buf_virt);
                desc.length  = 0;
                desc.cso     = 0;
                desc.cmd     = 0;
                // Mark all TX descriptors as "done" so the ring appears empty
                desc.status  = STATUS_DD;
                desc.css     = 0;
                desc.special = 0;
            }

            let ring_phys = virt_to_phys64(TX_DESC_RING.0.as_ptr() as u64);
            self.write_reg(REG_TDBAL, (ring_phys & 0xFFFF_FFFF) as u32);
            self.write_reg(REG_TDBAH, (ring_phys >> 32) as u32);
            self.write_reg(REG_TDLEN, (NUM_TX_DESC * DESC_SIZE) as u32);
            self.write_reg(REG_TDH, 0);
            self.write_reg(REG_TDT, 0);
            self.tx_tail = 0;
        }
    }
}

// ---------------------------------------------------------------------------
// Packet I/O
// ---------------------------------------------------------------------------

impl E1000e {
    pub fn receive_packet(&mut self) -> Option<Vec<u8>> {
        unsafe {
            let ring_ptr = RX_DESC_RING.0.as_mut_ptr() as *mut RxDescriptor;
            let desc = &mut *ring_ptr.add(self.rx_tail as usize);

            // Check the hardware "descriptor done" bit
            if desc.status & STATUS_DD == 0 {
                return None;
            }

            let length = desc.length as usize;

            // Silently drop error frames
            if desc.errors != 0 || length == 0 {
                // Give the descriptor back to the NIC and advance
                desc.status = 0;
                let old_tail = self.rx_tail;
                self.rx_tail = ((self.rx_tail as usize + 1) % NUM_RX_DESC) as u16;
                self.write_reg(REG_RDT, old_tail as u32);
                return None;
            }

            // Copy payload out of the static buffer
            let bufs_ptr = RX_PACKET_BUFS.0.as_ptr();
            let src = core::slice::from_raw_parts(
                bufs_ptr.add(self.rx_tail as usize * PACKET_BUF_SIZE),
                length,
            );
            let packet = src.to_vec();

            // Return descriptor to NIC
            desc.status = 0;
            let old_tail = self.rx_tail;
            self.rx_tail = ((self.rx_tail as usize + 1) % NUM_RX_DESC) as u16;
            self.write_reg(REG_RDT, old_tail as u32);

            Some(packet)
        }
    }

    pub fn transmit_packet(&mut self, payload: &[u8]) {
        let len = payload.len();
        if len > PACKET_BUF_SIZE {
            serial_println!("e1000e: transmit_packet: packet too large ({}), dropping", len);
            return;
        }

        unsafe {
            let ring_ptr = TX_DESC_RING.0.as_mut_ptr() as *mut TxDescriptor;
            let desc = &mut *ring_ptr.add(self.tx_tail as usize);

            // If DD is clear the ring is full — drop the packet
            if desc.status & STATUS_DD == 0 {
                serial_println!("e1000e: TX ring full, dropping packet");
                return;
            }

            // Copy payload into the pre-allocated TX buffer
            let bufs_ptr = TX_PACKET_BUFS.0.as_mut_ptr();
            let dst = core::slice::from_raw_parts_mut(
                bufs_ptr.add(self.tx_tail as usize * PACKET_BUF_SIZE),
                len,
            );
            dst.copy_from_slice(payload);

            // Fill in descriptor — buffer_addr was set during init and doesn't change
            desc.length  = len as u16;
            desc.cso     = 0;
            desc.cmd     = TX_CMD_EOP | TX_CMD_IFCS | TX_CMD_RS;
            desc.status  = 0; // clear DD so NIC knows to send
            desc.css     = 0;
            desc.special = 0;

            // Advance tail and ring the doorbell
            self.tx_tail = ((self.tx_tail as usize + 1) % NUM_TX_DESC) as u16;
            self.write_reg(REG_TDT, self.tx_tail as u32);
        }
    }
}

// ---------------------------------------------------------------------------
// NicDriver trait
// ---------------------------------------------------------------------------

impl super::nic::NicDriver for E1000e {
    fn mac_address(&self) -> [u8; 6] {
        self.mac_address
    }

    fn receive_packet(&mut self) -> Option<Vec<u8>> {
        self.receive_packet()
    }

    fn transmit_packet(&mut self, data: &[u8]) {
        self.transmit_packet(data);
    }

    fn capabilities(&self) -> smoltcp::phy::DeviceCapabilities {
        let mut caps = smoltcp::phy::DeviceCapabilities::default();
        caps.max_transmission_unit = 1500;
        caps.max_burst_size = Some(1);
        caps.medium = smoltcp::phy::Medium::Ethernet;
        caps
    }
}

// ---------------------------------------------------------------------------
// PCI init
// ---------------------------------------------------------------------------

pub fn init() -> Option<E1000e> {
    serial_println!("Scanning PCI bus for Intel I219 (e1000e)...");
    let devices = scan_pci();
    for dev in &devices {
        if dev.vendor_id == INTEL_VENDOR && I219_DEVICE_IDS.contains(&dev.device_id) {
            serial_println!(
                "==> MATCHED Intel e1000e (device ID 0x{:04X})",
                dev.device_id
            );
            if let Some(nic) = E1000e::new(dev) {
                return Some(nic);
            }
        }
    }
    None
}
