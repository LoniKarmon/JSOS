use x86_64::instructions::port::{Port, PortWriteOnly};
use crate::pci::{PciDevice, scan_pci, enable_bus_mastering};
use crate::serial_println;
use alloc::vec::Vec;
use alloc::vec;
use smoltcp::phy::{DeviceCapabilities, Medium};

const RTL8139_VENDOR_ID: u16 = 0x10EC;
const RTL8139_DEVICE_ID: u16 = 0x8139;

// Registers relative to IO base
const REG_TX_STATUS_0: u16 = 0x10;
const REG_TX_ADDR_0: u16 = 0x20;
const REG_RX_BUF_ADDR: u16 = 0x30;
const REG_COMMAND: u16 = 0x37; // 8-bit
const REG_CAPR: u16 = 0x38;    // Current Address of Packet Read, 16-bit
const REG_INTR_MASK: u16 = 0x3C; // 16-bit
const REG_RX_CONFIG: u16 = 0x44; // 32-bit
const REG_CONFIG_1: u16 = 0x52; // 8-bit

// Sizes
const RX_BUF_SIZE: usize = 8192 + 16 + 1500; // 8K + 16 bytes for padding + 1.5K for wrap
const RX_BASE_BUF_SIZE: usize = 8192;
const TX_BUF_SIZE: usize = 1536; // MTU + padding

pub struct Rtl8139 {
    io_base: u16,
    mac_address: [u8; 6],
    rx_buffer: &'static mut [u8],
    tx_buffers: [&'static mut [u8]; 4],
    tx_curr: usize,
    rx_curr: u16,
}

#[repr(align(4096))]
struct RxBufferWrapper([u8; RX_BUF_SIZE]);
static mut RTL_RX_BUFFER: RxBufferWrapper = RxBufferWrapper([0; RX_BUF_SIZE]);

#[repr(align(4096))]
struct TxBufferWrapper([u8; TX_BUF_SIZE]);
static mut RTL_TX_BUFFERS: [TxBufferWrapper; 4] = [
    TxBufferWrapper([0; TX_BUF_SIZE]),
    TxBufferWrapper([0; TX_BUF_SIZE]),
    TxBufferWrapper([0; TX_BUF_SIZE]),
    TxBufferWrapper([0; TX_BUF_SIZE]),
];

impl Rtl8139 {
    pub fn new(device: &PciDevice) -> Option<Self> {
        let io_base = device.get_bar_io_port(0);

        if io_base == 0 {
            serial_println!("RTL8139: No valid I/O base address found in BAR0.");
            return None;
        }

        enable_bus_mastering(device);

        let mut mac_address = [0u8; 6];
        unsafe {
            for i in 0..6 {
                let mut port = Port::<u8>::new(io_base + i);
                mac_address[i as usize] = port.read();
            }
        }

        let rx_buffer;
        let tx_buffers;
        unsafe {
            rx_buffer = &mut RTL_RX_BUFFER.0[..];
            tx_buffers = [
                &mut RTL_TX_BUFFERS[0].0[..],
                &mut RTL_TX_BUFFERS[1].0[..],
                &mut RTL_TX_BUFFERS[2].0[..],
                &mut RTL_TX_BUFFERS[3].0[..],
            ];
        }

        let nic = Rtl8139 {
            io_base,
            mac_address,
            rx_buffer,
            tx_buffers,
            tx_curr: 0,
            rx_curr: 0,
        };

        nic.turn_on();
        nic.software_reset();
        nic.init_buffers();
        nic.enable_interrupts();
        nic.enable_rx_tx();

        serial_println!("RTL8139: Init complete. MAC: {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
            mac_address[0], mac_address[1], mac_address[2],
            mac_address[3], mac_address[4], mac_address[5]);

        Some(nic)
    }

    pub fn mac_address(&self) -> [u8; 6] {
        self.mac_address
    }

    fn outb(&self, offset: u16, data: u8) {
        unsafe { PortWriteOnly::<u8>::new(self.io_base + offset).write(data); }
    }

    fn outw(&self, offset: u16, data: u16) {
        unsafe { PortWriteOnly::<u16>::new(self.io_base + offset).write(data); }
    }

    fn outl(&self, offset: u16, data: u32) {
        unsafe { PortWriteOnly::<u32>::new(self.io_base + offset).write(data); }
    }

    fn turn_on(&self) {
        // Power on the device
        self.outb(REG_CONFIG_1, 0x00);
    }

    fn software_reset(&self) {
        // Issue Soft Reset
        self.outb(REG_COMMAND, 0x10);
        // Wait for reset to clear
        while unsafe { Port::<u8>::new(self.io_base + REG_COMMAND).read() } & 0x10 != 0 {
            core::hint::spin_loop();
        }
    }

    fn init_buffers(&self) {
        // Set RX Buffer memory location (physical address via OS mapping)
        let rx_phys = crate::memory::virt_to_phys(self.rx_buffer.as_ptr() as u64);
        self.outl(REG_RX_BUF_ADDR, rx_phys);
    }

    fn enable_interrupts(&self) {
        // TOK, ROK, TER, RER
        // 0x0005 = Receive OK, Transmit OK
        self.outw(REG_INTR_MASK, 0x0005);
    }

    fn enable_rx_tx(&self) {
        // Accept Broadcast, Multicast, Physical Match, Wrap
        // AB+AM+APM+AAP = 0x0F
        // WRAP = 0x80
        self.outl(REG_RX_CONFIG, 0x0F | 0x80);
        
        // Enable RX and TX
        self.outb(REG_COMMAND, 0x0C); // RE=0x08, TE=0x04
    }

    pub fn receive_packet(&mut self) -> Option<Vec<u8>> {
        let cmd = unsafe { Port::<u8>::new(self.io_base + REG_COMMAND).read() };
        // crate::serial_println!("RTL8139: receive_packet => cmd: 0x{:02X}", cmd);
        if cmd & 0x01 != 0 {
            // Buffer is empty
            return None;
        }

        let rx_curr = self.rx_curr as usize;
        let header = u16::from_le_bytes([self.rx_buffer[rx_curr], self.rx_buffer[rx_curr + 1]]);
        let length = u16::from_le_bytes([self.rx_buffer[rx_curr + 2], self.rx_buffer[rx_curr + 3]]) as usize;
        // crate::serial_println!("RTL8139: rx_curr: {}, header: 0x{:04X}, len: {}", rx_curr, header, length);

        if (header & 1) == 0 {
            // ROK bit not set, packet error
            // Need to reset the card logic or ignore, skipping for simplicity
            return None;
        }

        // The length includes the 4 byte CRC
        let packet_len = length.saturating_sub(4);
        let mut packet = vec![0u8; packet_len];

        // The packet data starts at rx_curr + 4
        let rx_read_ptr = rx_curr + 4;
        
        for i in 0..packet_len {
            packet[i] = self.rx_buffer[(rx_read_ptr + i) % RX_BUF_SIZE];
        }

        // Update read pointer: aligned to 4 bytes boundary
        self.rx_curr = ((rx_curr + length + 4 + 3) & !3) as u16 % RX_BASE_BUF_SIZE as u16;
        
        // Write the current read pointer minus 16 to CAPR (Card convention)
        self.outw(REG_CAPR, self.rx_curr.wrapping_sub(16));

        Some(packet)
    }

    pub fn transmit_packet(&mut self, payload: &[u8]) {
        let tx_id = self.tx_curr;
        let len = payload.len();
        // crate::serial_println!("RTL8139: transmit_packet => len: {}", len);
        
        // Copy packet into tx buffer
        self.tx_buffers[tx_id][..len].copy_from_slice(payload);
        
        // Write physical address to TX_ADDR_x via OS memory bridge
        let tx_phys = crate::memory::virt_to_phys(self.tx_buffers[tx_id].as_ptr() as u64);
        self.outl(REG_TX_ADDR_0 + (tx_id as u16 * 4), tx_phys);
        
        // Write status/length to TX_STATUS_x (triggers send)
        self.outl(REG_TX_STATUS_0 + (tx_id as u16 * 4), len as u32);
        
        // Increment round-robin index
        self.tx_curr = (self.tx_curr + 1) % 4;
    }
}

impl super::nic::NicDriver for Rtl8139 {
    fn mac_address(&self) -> [u8; 6] {
        self.mac_address
    }
    fn receive_packet(&mut self) -> Option<Vec<u8>> {
        self.receive_packet()
    }
    fn transmit_packet(&mut self, data: &[u8]) {
        self.transmit_packet(data);
    }
    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.max_transmission_unit = 1500;
        caps.max_burst_size = Some(1);
        caps.medium = Medium::Ethernet;
        caps
    }
}

pub fn init() -> Option<Rtl8139> {
    serial_println!("Scanning PCI bus for RTL8139...");
    let devices = scan_pci();
    serial_println!("Found {} total PCI devices attached.", devices.len());

    for dev in &devices {
        serial_println!("PCI Device [Vendor: 0x{:04X}, Device: 0x{:04X}] at bus {} device {}",
            dev.vendor_id, dev.device_id, dev.bus, dev.device);
        if dev.vendor_id == RTL8139_VENDOR_ID && dev.device_id == RTL8139_DEVICE_ID {
            serial_println!("==> MATCHED RTL8139 PCI device!");
            if let Some(nic) = Rtl8139::new(dev) {
                return Some(nic);
            }
        }
    }
    None
}
