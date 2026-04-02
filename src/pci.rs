use x86_64::instructions::port::{Port, PortWriteOnly};

const CONFIG_ADDRESS: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;

pub struct PciDevice {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub header_type: u8,
    pub irq_line: u8,
}

pub const PCI_CLASS_SERIAL: u8 = 0x0C;
pub const PCI_SUBCLASS_USB: u8 = 0x03;
pub const PCI_PROGIF_XHCI: u8 = 0x30;


impl PciDevice {
    pub fn read_word(bus: u8, slot: u8, func: u8, offset: u8) -> u16 {
        let address: u32 = 0x80000000
            | ((bus as u32) << 16)
            | ((slot as u32) << 11)
            | ((func as u32) << 8)
            | (offset as u32 & 0xFC);
        
        unsafe {
            let mut addr_port = PortWriteOnly::<u32>::new(CONFIG_ADDRESS);
            let mut data_port = Port::<u32>::new(CONFIG_DATA);
            
            addr_port.write(address);
            ((data_port.read() >> ((offset & 2) * 8)) & 0xFFFF) as u16
        }
    }
    
    pub fn read_dword(bus: u8, slot: u8, func: u8, offset: u8) -> u32 {
        let address: u32 = 0x80000000
            | ((bus as u32) << 16)
            | ((slot as u32) << 11)
            | ((func as u32) << 8)
            | (offset as u32 & 0xFC);
        
        unsafe {
            let mut addr_port = PortWriteOnly::<u32>::new(CONFIG_ADDRESS);
            let mut data_port = Port::<u32>::new(CONFIG_DATA);
            
            addr_port.write(address);
            data_port.read()
        }
    }

    pub fn read_bar(&self, bar_index: u8) -> u32 {
        let offset = 0x10 + (bar_index * 4);
        Self::read_dword(self.bus, self.device, self.function, offset)
    }

    pub fn is_bar_io(&self, bar_index: u8) -> bool {
        (self.read_bar(bar_index) & 1) == 1
    }

    pub fn get_bar_io_port(&self, bar_index: u8) -> u16 {
        (self.read_bar(bar_index) & !3) as u16
    }
    
    pub fn get_bar_memory_address(&self, bar_index: u8) -> u32 {
        self.read_bar(bar_index) & !0xF
    }

    fn check_device(bus: u8, device: u8, function: u8) -> Option<PciDevice> {
        let vendor_id = Self::read_word(bus, device, function, 0);
        if vendor_id == 0xFFFF {
            return None; // Device doesn't exist
        }

        let device_id = Self::read_word(bus, device, function, 2);
        
        let class_word = Self::read_word(bus, device, function, 0x0A);
        let class = (class_word >> 8) as u8;
        let subclass = (class_word & 0xFF) as u8;
        
        let header_word = Self::read_word(bus, device, function, 0x0E);
        let header_type = (header_word & 0xFF) as u8;
        
        let prog_if_word = Self::read_word(bus, device, function, 0x08);
        let prog_if = (prog_if_word >> 8) as u8;

        let irq_word = Self::read_word(bus, device, function, 0x3C);
        let irq_line = (irq_word & 0xFF) as u8;

        Some(PciDevice {
            bus,
            device,
            function,
            vendor_id,
            device_id,
            class,
            subclass,
            prog_if,
            header_type,
            irq_line,
        })
    }
}

pub fn scan_pci() -> alloc::vec::Vec<PciDevice> {
    let mut devices = alloc::vec::Vec::new();

    // Brute force scan standard PCI buses
    for bus in 0..=255 {
        for device in 0..32 {
            if let Some(pci_dev) = PciDevice::check_device(bus, device, 0) {
                // If it's a multi-function device, scan the rest
                let is_multi_function = (pci_dev.header_type & 0x80) != 0;
                devices.push(pci_dev);

                if is_multi_function {
                    for function in 1..8 {
                        if let Some(func_dev) = PciDevice::check_device(bus, device, function) {
                            devices.push(func_dev);
                        }
                    }
                }
            }
        }
    }
    
    devices
}

pub fn find_xhci_controllers() -> alloc::vec::Vec<PciDevice> {
    scan_pci()
        .into_iter()
        .filter(|dev| {
            dev.class == PCI_CLASS_SERIAL
                && dev.subclass == PCI_SUBCLASS_USB
                && dev.prog_if == PCI_PROGIF_XHCI
        })
        .collect()
}


pub fn enable_bus_mastering(device: &PciDevice) {
    let offset = 0x04;
    let command_read = PciDevice::read_word(device.bus, device.device, device.function, offset);
    
    // Set Bus Master bit (bit 2)
    let command_write = command_read | (1 << 2);
    
    let address: u32 = 0x80000000
        | ((device.bus as u32) << 16)
        | ((device.device as u32) << 11)
        | ((device.function as u32) << 8)
        | (offset as u32 & 0xFC);

    // Keep the high bits unchanged when doing a dword write for a word register
    let current_dword = PciDevice::read_dword(device.bus, device.device, device.function, offset);
    let mut new_dword = current_dword;
    
    // offset 0x04 starts at bit 0 of the dword
    new_dword = (new_dword & 0xFFFF0000) | (command_write as u32);
    
    unsafe {
        let mut addr_port = PortWriteOnly::<u32>::new(CONFIG_ADDRESS);
        let mut data_port = PortWriteOnly::<u32>::new(CONFIG_DATA);
        addr_port.write(address);
        data_port.write(new_dword);
    }
}
