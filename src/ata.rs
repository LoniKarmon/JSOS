// src/ata.rs — Legacy ATA PIO Mode IDE Driver
// Interacts with the primary IDE controller (0x1F0-0x1F7)

use x86_64::instructions::port::{Port, PortReadOnly, PortWriteOnly};
use spin::Mutex;
use lazy_static::lazy_static;

const ATA_PRIMARY_IO: u16 = 0x1F0;
const ATA_PRIMARY_CTRL: u16 = 0x3F6;

struct AtaPorts {
    data: Port<u16>,
    error: PortReadOnly<u8>,
    features: PortWriteOnly<u8>,
    sector_count: Port<u8>,
    lba_lo: Port<u8>,
    lba_mid: Port<u8>,
    lba_hi: Port<u8>,
    drive_head: Port<u8>,
    status: PortReadOnly<u8>,
    command: PortWriteOnly<u8>,
    alt_status: PortReadOnly<u8>,
}

impl AtaPorts {
    fn new(base: u16, ctrl: u16) -> Self {
        Self {
            data: Port::new(base + 0),
            error: PortReadOnly::new(base + 1),
            features: PortWriteOnly::new(base + 1),
            sector_count: Port::new(base + 2),
            lba_lo: Port::new(base + 3),
            lba_mid: Port::new(base + 4),
            lba_hi: Port::new(base + 5),
            drive_head: Port::new(base + 6),
            status: PortReadOnly::new(base + 7),
            command: PortWriteOnly::new(base + 7),
            alt_status: PortReadOnly::new(ctrl),
        }
    }
}

pub struct AtaDrive {
    ports: AtaPorts,
    is_slave: bool,
}

lazy_static! {
    pub static ref PRIMARY_DRIVE: Mutex<AtaDrive> = Mutex::new(AtaDrive {
        ports: AtaPorts::new(ATA_PRIMARY_IO, ATA_PRIMARY_CTRL),
        // Drive 0 is the master (boot drive), Drive 1 is the slave (JSKV data drive)
        is_slave: true,
    });
}

impl AtaDrive {
    /// Wait until the drive is not busy, checking for errors. Returns true if ready, false on error.
    fn wait_ready(&mut self) -> bool {
        for _ in 0..100000 {
            let status = unsafe { self.ports.status.read() };
            if status & 0x80 == 0 { // BSY bit cleared
                if status & 0x01 != 0 || status & 0x20 != 0 {
                    // ERR or DF bit set
                    return false;
                }
                if status & 0x08 != 0 {
                    // DRQ bit set (ready for data)
                    return true;
                }
            }
            core::hint::spin_loop();
        }
        false
    }
    
    /// Wait until BSY is clear before sending a command
    fn wait_bsy_clear(&mut self) {
        for _ in 0..100000 {
            let status = unsafe { self.ports.status.read() };
            if status & 0x80 == 0 {
                return;
            }
            core::hint::spin_loop();
        }
    }

    /// Read a single 512-byte sector at the given LBA (28-bit)
    pub fn read_sector(&mut self, lba: u32, buf: &mut [u8; 512]) -> bool {
        let drive_sel = if self.is_slave { 0xF0 } else { 0xE0 }; // 0xE0 = Master, 0xF0 = Slave
        
        unsafe {
            self.wait_bsy_clear();

            self.ports.drive_head.write(drive_sel | ((lba >> 24) & 0x0F) as u8);
            
            // Wait slightly for drive select to process
            for _ in 0..4 { self.ports.alt_status.read(); }

            self.ports.features.write(0);
            self.ports.sector_count.write(1);
            self.ports.lba_lo.write((lba & 0xFF) as u8);
            self.ports.lba_mid.write(((lba >> 8) & 0xFF) as u8);
            self.ports.lba_hi.write(((lba >> 16) & 0xFF) as u8);

            // Command 0x20 = Read Sectors
            self.ports.command.write(0x20);
        }

        if !self.wait_ready() {
            let err = unsafe { self.ports.error.read() };
            crate::serial_println!("[ATA] Read error at LBA {}. Error reg: {:#04x}", lba, err);
            return false;
        }

        // Read 256 words (512 bytes)
        for i in 0..256 {
            let word = unsafe { self.ports.data.read() };
            buf[i * 2] = (word & 0xFF) as u8;
            buf[i * 2 + 1] = (word >> 8) as u8;
        }

        true
    }

    /// Write a single 512-byte sector at the given LBA (28-bit)
    pub fn write_sector(&mut self, lba: u32, buf: &[u8; 512]) -> bool {
        let drive_sel = if self.is_slave { 0xF0 } else { 0xE0 };
        
        unsafe {
            self.wait_bsy_clear();

            self.ports.drive_head.write(drive_sel | ((lba >> 24) & 0x0F) as u8);
            
            // Wait
            for _ in 0..4 { self.ports.alt_status.read(); }

            self.ports.features.write(0);
            self.ports.sector_count.write(1);
            self.ports.lba_lo.write((lba & 0xFF) as u8);
            self.ports.lba_mid.write(((lba >> 8) & 0xFF) as u8);
            self.ports.lba_hi.write(((lba >> 16) & 0xFF) as u8);

            // Command 0x30 = Write Sectors
            self.ports.command.write(0x30);
        }

        if !self.wait_ready() {
            let err = unsafe { self.ports.error.read() };
            crate::serial_println!("[ATA] Write error at LBA {}. Error reg: {:#04x}", lba, err);
            return false;
        }

        // Write 256 words (512 bytes)
        for i in 0..256 {
            let word = (buf[i * 2] as u16) | ((buf[i * 2 + 1] as u16) << 8);
            unsafe { self.ports.data.write(word) };
        }

        // Flush cache (Command 0xE7)
        unsafe { self.ports.command.write(0xE7); }
        self.wait_bsy_clear();

        true
    }
}
