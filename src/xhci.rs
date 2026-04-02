use crate::pci::{PciDevice, enable_bus_mastering};
use crate::memory::{PHYS_MEM_OFFSET, virt_to_phys};
use core::sync::atomic::Ordering;
use x86_64::VirtAddr;
use alloc::vec::Vec;
use crate::serial_println;
use core::alloc::Layout;
use lazy_static::lazy_static;
use spin::Mutex;

// Helper: read a u32 from a MMIO address (base + offset in bytes)
#[inline(always)]
unsafe fn mmio_read32(base: VirtAddr, offset: u64) -> u32 {
    core::ptr::read_volatile((base.as_u64() + offset) as *const u32)
}

// Helper: write a u32 to a MMIO address (base + offset in bytes)
#[inline(always)]
unsafe fn mmio_write32(base: VirtAddr, offset: u64, val: u32) {
    core::ptr::write_volatile((base.as_u64() + offset) as *mut u32, val)
}

// Helper: read a u16 from a MMIO address
#[inline(always)]
unsafe fn mmio_read16(base: VirtAddr, offset: u64) -> u16 {
    core::ptr::read_volatile((base.as_u64() + offset) as *const u16)
}

// Helper: read a u8 from a MMIO address
#[inline(always)]
unsafe fn mmio_read8(base: VirtAddr, offset: u64) -> u8 {
    core::ptr::read_volatile((base.as_u64() + offset) as *const u8)
}

/// A helper for allocating DMA-safe, aligned memory.
pub struct Dma<T> {
    pub virt: *mut T,
    pub phys: u32,
    layout: Layout,
}

// Safety: Dma holds a raw pointer to kernel-owned DMA memory.
// We never share or alias through this pointer from multiple threads.
unsafe impl<T> Send for Dma<T> {}

impl<T> Dma<T> {
    pub fn new_zeroed(alignment: usize) -> Self {
        let layout = Layout::from_size_align(core::mem::size_of::<T>(), alignment).unwrap();
        let virt = unsafe {
            let ptr = alloc::alloc::alloc_zeroed(layout) as *mut T;
            if ptr.is_null() {
                panic!("DMA allocation failed: {:?}", layout);
            }
            ptr
        };
        let phys = virt_to_phys(virt as u64);
        Self { virt, phys, layout }
    }
}

impl<T> Drop for Dma<T> {
    fn drop(&mut self) {
        unsafe {
            alloc::alloc::dealloc(self.virt as *mut u8, self.layout);
        }
    }
}

#[repr(C, align(16))]
#[derive(Copy, Clone, Debug)]
pub struct Trb {
    pub data_low: u32,
    pub data_high: u32,
    pub status: u32,
    pub control: u32,
}

impl Trb {
    pub fn new() -> Self {
        Self { data_low: 0, data_high: 0, status: 0, control: 0 }
    }
}

pub struct XhciController {
    pub pci_device: PciDevice,
    pub cap_base: VirtAddr,
    pub op_base: VirtAddr,
    pub db_base: VirtAddr,
    pub runtime_base: VirtAddr,
    pub dcbaa: Option<Dma<[u64; 256]>>,
    pub cmd_ring: Option<Dma<[Trb; 16]>>,
    pub event_ring: Option<Dma<[Trb; 16]>>,
    pub erst: Option<Dma<[u64; 2]>>,
    pub event_idx: usize,
    pub event_cycle: u32,
    pub ports_addressed: u32, // Bitmask of addressed ports
    pub hid_devices: Vec<HidDevice>,
    pub pending_command_completion: Option<Trb>,
    pub pending_transfer_completion: Option<Trb>,
    pub needs_monitor: bool,
    pub context_size: usize, // 32 or 64
    pub cmd_idx: usize,
    pub cmd_cycle: u32,
    pub device_contexts: Vec<Option<Dma<[u32; 512]>>>,
    pub ep0_rings: Vec<Option<Dma<[Trb; 16]>>>,
    pub scratchpad_array: Option<Dma<[u64; 64]>>,
    pub scratchpad_pages: Vec<Dma<[u8; 4096]>>,
}

#[derive(Copy, Clone, PartialEq)]
pub enum DeviceKind {
    Keyboard,
    Mouse,
}

pub struct HidDevice {
    pub slot_id: u8,
    pub ep_index: u8,
    pub kind: DeviceKind,
    pub ring: Dma<[Trb; 16]>,
    pub buffer: Dma<[u8; 8]>,
    pub pending: bool,
    pub prev_report: [u8; 8],
    pub ring_idx: usize,
    pub ring_cycle: u32,
    // Key repeat state (keyboards only)
    pub repeat_key: u8,       // HID keycode being held; 0 = none
    pub repeat_next_tick: u64, // TICKS value when the next repeat fires
}

impl XhciController {
    pub fn new(pci_device: PciDevice) -> Self {
        enable_bus_mastering(&pci_device);

        let bar0 = pci_device.read_bar(0);
        let base_phys = if (bar0 & 0x6) == 0x4 {
            let bar1 = pci_device.read_bar(1);
            ((bar1 as u64) << 32) | (bar0 as u64 & !0xF)
        } else {
            bar0 as u64 & !0xF
        };

        let phys_offset = PHYS_MEM_OFFSET.load(Ordering::Relaxed);
        let cap_base = VirtAddr::new(base_phys + phys_offset);

        let cap_length = unsafe { mmio_read8(cap_base, 0) };
        let op_base = VirtAddr::new(cap_base.as_u64() + cap_length as u64);

        let db_offset = unsafe { mmio_read32(cap_base, 0x14) };
        let runtime_offset = unsafe { mmio_read32(cap_base, 0x18) };
        
        // FIX: HCCPARAMS1 is at offset 0x10. 
        let hcc_params1 = unsafe { mmio_read32(cap_base, 0x10) };
        let context_size = if (hcc_params1 & 4) != 0 { 64 } else { 32 };

        Self {
            pci_device,
            cap_base,
            op_base,
            db_base: VirtAddr::new(cap_base.as_u64() + db_offset as u64),
            runtime_base: VirtAddr::new(cap_base.as_u64() + runtime_offset as u64),
            dcbaa: None,
            cmd_ring: None,
            event_ring: None,
            erst: None,
            event_idx: 0,
            event_cycle: 1, 
            ports_addressed: 0,
            hid_devices: Vec::new(),
            pending_command_completion: None,
            pending_transfer_completion: None,
            needs_monitor: false,
            context_size,
            cmd_idx: 0,
            cmd_cycle: 1,
            device_contexts: Vec::new(),
            ep0_rings: Vec::new(),
            scratchpad_array: None,
            scratchpad_pages: Vec::new(),
        }
    }
    
pub fn set_configuration(&mut self, slot_id: u8, config_value: u8) {
        crate::serial_println!("[xhci] SET_CONFIGURATION {} for slot {}", config_value, slot_id);

        let setup_data: u64 = ((config_value as u64) << 16) | 0x0900;
        let mut setup_trb = Trb::new();
        setup_trb.data_low  = setup_data as u32;
        setup_trb.data_high = (setup_data >> 32) as u32;
        setup_trb.status    = 8; 
        setup_trb.control   = (2 << 10) | (0 << 16) | (1 << 6) | 1; 

        let mut status_trb = Trb::new();
        // Type 4 (Status), DIR=1 (IN for No Data stage), IOC=1, C=1
        status_trb.control = (4 << 10) | (1 << 16) | (1 << 5) | 1; 

        if let Some(ep0_ring) = &self.ep0_rings[slot_id as usize] {
            unsafe {
                let ptr = ep0_ring.virt as *mut Trb;
                // Move indices to 6, 7
                ptr.add(6).write_volatile(setup_trb);
                ptr.add(7).write_volatile(status_trb);
                
                core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::Release);
                core::arch::asm!("sfence", options(nostack, preserves_flags));
            }
        }

        unsafe {
            let db_ptr = (self.db_base.as_u64() + (slot_id as u64) * 4) as *mut u32;
            db_ptr.write_volatile(1);
        }
        
        if let Some(event) = self.poll_event() {
            let code = (event.status >> 24) & 0xFF;
            crate::serial_println!("[xhci] SET_CONFIGURATION result code: {}", code);
        } else {
            crate::serial_println!("[xhci] SET_CONFIGURATION TIMEOUT");
        }
    }

    pub fn configure_hid_endpoint(&mut self, slot_id: u8, ep_addr: u8, max_packet_size: u16, interval: u8, kind: DeviceKind) {
        crate::serial_println!("[xhci] Configuring HID Endpoint 0x{:02x} (MPS={}, interval={})", ep_addr, max_packet_size, interval);
        
        let dci = ((ep_addr & 0xF) as usize * 2) + (if (ep_addr & 0x80) != 0 { 1 } else { 0 });
        let ring: Dma<[Trb; 16]> = Dma::new_zeroed(64);
        let buffer: Dma<[u8; 8]> = Dma::new_zeroed(64);

        let mut input_ctx: Dma<[u32; 1024]> = Dma::new_zeroed(64);
        
        unsafe {
            let ptr = input_ctx.virt as *mut u32;

            ptr.add(self.ctx_offset(0) + 1).write_volatile((1 << dci) | (1 << 0)); 

            if let Some(dev_ctx) = &self.device_contexts[slot_id as usize] {
                core::ptr::copy_nonoverlapping(
                    (dev_ctx.virt as *const u32).add(self.ctx_offset(0)),
                    ptr.add(self.ctx_offset(1)),
                    self.context_size / 4
                );
            }

            let slot_dw0 = ptr.add(self.ctx_offset(1) + 0);
            let mut val = slot_dw0.read_volatile();
            val &= !(0x1F << 27); 
            val |= (dci as u32) << 27; 
            slot_dw0.write_volatile(val);

            // Input context layout: index 0 = ICC, index 1 = Slot, index N+1 = DCI=N endpoint.
            // Device context layout (no ICC): index 0 = Slot, index N = DCI=N endpoint.
            // So endpoint DCI=N must be written at input context index N+1.
            let ep_off = self.ctx_offset(dci + 1);
            ptr.add(ep_off + 0).write_volatile((interval as u32) << 16);
            
            // Inject the dynamic Max Packet Size into the endpoint context
            ptr.add(ep_off + 1).write_volatile((3 << 1) | (7 << 3) | ((max_packet_size as u32) << 16));
            ptr.add(ep_off + 2).write_volatile(ring.phys | 1);
            ptr.add(ep_off + 4).write_volatile(8);
        }

        let mut trb = Trb::new();
        trb.data_low = input_ctx.phys;
        trb.control = (12 << 10) | ((slot_id as u32) << 24); 
        self.send_command(trb);

        if let Some(event) = self.poll_event() {
            let code = (event.status >> 24) & 0xFF;
            crate::serial_println!("[xhci] Configure Endpoint code: {}", code);
            if code == 1 {
                self.hid_devices.push(HidDevice {
                    slot_id,
                    ep_index: dci as u8,
                    kind,
                    ring,
                    buffer,
                    pending: false,
                    prev_report: [0; 8],
                    ring_idx: 0,
                    ring_cycle: 1,
                    repeat_key: 0,
                    repeat_next_tick: 0,
                });

                self.set_configuration(slot_id, 1);
                
                // NEW: Initialize the keyboard state machine!
                self.set_hid_protocol_and_idle(slot_id);
            }
        }
    }

    pub fn set_hid_protocol_and_idle(&mut self, slot_id: u8) {
        crate::serial_println!("[xhci] Setting HID Protocol and Idle for slot {}", slot_id);

        // 1. SET_IDLE
        let mut setup_trb = Trb::new();
        setup_trb.data_low  = 0x0000_0A21; // bmReq=0x21, bReq=0x0A (SET_IDLE), wValue=0
        setup_trb.data_high = 0;
        setup_trb.status    = 8; 
        setup_trb.control   = (2 << 10) | (0 << 16) | (1 << 6) | 1; 

        let mut status_trb = Trb::new();
        status_trb.control = (4 << 10) | (1 << 16) | (1 << 5) | 1; 

        if let Some(ep0_ring) = &self.ep0_rings[slot_id as usize] {
            unsafe {
                let ptr = ep0_ring.virt as *mut Trb;
                ptr.add(8).write_volatile(setup_trb);
                ptr.add(9).write_volatile(status_trb);
                core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::Release);
                core::arch::asm!("sfence", options(nostack, preserves_flags));
            }
        }

        unsafe {
            let db_ptr = (self.db_base.as_u64() + (slot_id as u64) * 4) as *mut u32;
            db_ptr.write_volatile(1);
        }
        
        if let Some(event) = self.poll_event() {
            crate::serial_println!("[xhci] SET_IDLE result code: {}", (event.status >> 24) & 0xFF);
        }

        // 2. SET_PROTOCOL
        let mut setup_trb2 = Trb::new();
        setup_trb2.data_low  = 0x0000_0B21; // bmReq=0x21, bReq=0x0B (SET_PROTOCOL), wValue=0 (Boot Protocol)
        setup_trb2.data_high = 0;
        setup_trb2.status    = 8; 
        setup_trb2.control   = (2 << 10) | (0 << 16) | (1 << 6) | 1; 

        let mut status_trb2 = Trb::new();
        status_trb2.control = (4 << 10) | (1 << 16) | (1 << 5) | 1; 

        if let Some(ep0_ring) = &self.ep0_rings[slot_id as usize] {
            unsafe {
                let ptr = ep0_ring.virt as *mut Trb;
                ptr.add(10).write_volatile(setup_trb2);
                ptr.add(11).write_volatile(status_trb2);
                core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::Release);
                core::arch::asm!("sfence", options(nostack, preserves_flags));
            }
        }

        unsafe {
            let db_ptr = (self.db_base.as_u64() + (slot_id as u64) * 4) as *mut u32;
            db_ptr.write_volatile(1);
        }
        
        if let Some(event) = self.poll_event() {
            crate::serial_println!("[xhci] SET_PROTOCOL result code: {}", (event.status >> 24) & 0xFF);
        }
    }

    fn ctx_offset(&self, index: usize) -> usize {
        (self.context_size / 4) * index
    }

    /// Walk the xHCI Extended Capabilities list and claim OS ownership from the
    /// BIOS via the USB Legacy Support Capability (USBLEGSUP, ID=1).
    /// Without this handshake the BIOS SMM handler may still own the controller
    /// and interfere with our command ring writes.
    fn claim_os_ownership(&self) {
        let hcc_params1 = unsafe { mmio_read32(self.cap_base, 0x10) };
        let xecp_offset = ((hcc_params1 >> 16) & 0xFFFF) as u64 * 4;
        if xecp_offset == 0 { return; }

        let mut cap_off = xecp_offset;
        for _ in 0..256 {
            let cap = unsafe { mmio_read32(self.cap_base, cap_off) };
            let cap_id   = cap & 0xFF;
            let cap_next = ((cap >> 8) & 0xFF) as u64;

            if cap_id == 1 {
                // USB Legacy Support Capability found.
                // bit 16 = HC BIOS Owned Semaphore, bit 24 = HC OS Owned Semaphore.
                serial_println!("[xhci] USBLEGSUP @ cap+{:#x}: {:#010x}", cap_off, cap);
                // Set the OS owned bit.
                let new_cap = cap | (1 << 24);
                unsafe { mmio_write32(self.cap_base, cap_off, new_cap); }
                // Wait up to ~50 000 iterations for BIOS to release.
                for _ in 0..50_000 {
                    let v = unsafe { mmio_read32(self.cap_base, cap_off) };
                    if (v >> 16) & 1 == 0 {
                        serial_println!("[xhci] BIOS released ownership.");
                        // Clear USBLEGCTLSTS (cap_off+4) to disable all BIOS USB SMI enables.
                        // Without this, the BIOS SMM handler fires on every doorbell ring and
                        // intercepts our command ring, causing Enable Slot TIMEOUT.
                        let legctlsts = unsafe { mmio_read32(self.cap_base, cap_off + 4) };
                        serial_println!("[xhci] USBLEGCTLSTS before clear: {:#010x}", legctlsts);
                        unsafe { mmio_write32(self.cap_base, cap_off + 4, 0); }
                        serial_println!("[xhci] USBLEGCTLSTS cleared.");
                        return;
                    }
                    core::hint::spin_loop();
                }
                // BIOS didn't release — clear SMI enables anyway (best-effort).
                unsafe { mmio_write32(self.cap_base, cap_off + 4, 0); }
                serial_println!("[xhci] WARNING: BIOS did not release ownership — continuing anyway.");
                return;
            }

            if cap_next == 0 { break; }
            cap_off += cap_next * 4;
        }
    }

    pub fn init(&mut self) {
        serial_println!("[xhci] Initializing controller at PCI {:02x}:{:02x}.{}",
            self.pci_device.bus, self.pci_device.device, self.pci_device.function);

        let hcs_params1 = unsafe { mmio_read32(self.cap_base, 0x04) };
        let max_slots = hcs_params1 & 0xFF;
        let max_ports = (hcs_params1 >> 24) & 0xFF;
        serial_println!("[xhci] Max Slots: {}, Max Ports: {}", max_slots, max_ports);

        // Claim OS ownership BEFORE reset so the BIOS SMM handler doesn't
        // interfere with our command ring after we start the controller.
        self.claim_os_ownership();

        // Pre-populate the slot/ring Vecs so monitor_ports (called inside
        // setup_structures) can safely index into them even if PRC fires
        // during the first port scan (e.g. from a BIOS-initiated reset that
        // completed just before we started).
        self.device_contexts.clear();
        self.ep0_rings.clear();
        for _ in 0..(max_slots as usize + 1) {
            self.device_contexts.push(None);
            self.ep0_rings.push(None);
        }

        if !self.reset() {
            serial_println!("[xhci] Controller reset timed out — skipping init.");
            return;
        }
        // setup_structures starts the controller and calls monitor_ports once.
        self.setup_structures();

        // Second port scan: picks up any devices that connected while the
        // controller was coming up and whose CSC we may have missed.
        self.monitor_ports();
    }

    fn reset(&mut self) -> bool {
        if !self.wait_for_cnr(false) { return false; }
        let usbcmd = unsafe { mmio_read32(self.op_base, 0) };
        unsafe { mmio_write32(self.op_base, 0, usbcmd & !1) };
        if !self.wait_for_halt(true) { return false; }
        unsafe { mmio_write32(self.op_base, 0, usbcmd | 2) };
        if !self.wait_for_reset() { return false; }
        if !self.wait_for_cnr(false) { return false; }
        serial_println!("[xhci] Controller reset complete.");
        true
    }

    fn wait_for_cnr(&self, state: bool) -> bool {
        for _ in 0..1_000_000 {
            let usbsts = unsafe { mmio_read32(self.op_base, 4) };
            let cnr = (usbsts >> 11) & 1;
            if (cnr != 0) == state { return true; }
            core::hint::spin_loop();
        }
        serial_println!("[xhci] wait_for_cnr TIMEOUT");
        false
    }

    fn wait_for_halt(&self, state: bool) -> bool {
        for _ in 0..1_000_000 {
            let usbsts = unsafe { mmio_read32(self.op_base, 4) };
            let hch = usbsts & 1;
            if (hch != 0) == state { return true; }
            core::hint::spin_loop();
        }
        serial_println!("[xhci] wait_for_halt TIMEOUT");
        false
    }

    fn wait_for_reset(&self) -> bool {
        for _ in 0..1_000_000 {
            let usbcmd = unsafe { mmio_read32(self.op_base, 0) };
            if (usbcmd >> 1) & 1 == 0 { return true; }
            core::hint::spin_loop();
        }
        serial_println!("[xhci] wait_for_reset TIMEOUT");
        false
    }

    pub fn setup_structures(&mut self) {
        let mut dcbaa: Dma<[u64; 256]> = Dma::new_zeroed(64);

        // Scratchpad buffers: real hardware xHCI controllers require these to be
        // allocated before the controller is started (HCSPARAMS2 bits 31:21).
        // QEMU's virtual xHCI reports 0 and works without them; real hardware
        // will silently fail to process commands if they are missing.
        let hcsparams2 = unsafe { mmio_read32(self.cap_base, 0x08) };
        let max_sp = ((((hcsparams2 >> 27) & 0x1F) << 5) | ((hcsparams2 >> 21) & 0x1F)) as usize;
        serial_println!("[xhci] HCSPARAMS2={:#010x} Max Scratchpad Buffers={}", hcsparams2, max_sp);
        if max_sp > 0 {
            let capped = max_sp.min(64);
            let mut sp_array: Dma<[u64; 64]> = Dma::new_zeroed(64);
            for i in 0..capped {
                let page: Dma<[u8; 4096]> = Dma::new_zeroed(4096);
                unsafe { (*sp_array.virt)[i] = page.phys as u64; }
                self.scratchpad_pages.push(page);
            }
            // DCBAA[0] must point to the scratchpad buffer array.
            unsafe {
                core::ptr::write_volatile(
                    (dcbaa.virt as *mut u64).add(0),
                    sp_array.phys as u64,
                );
            }
            serial_println!("[xhci] Scratchpad array phys={:#010x}, {} pages allocated",
                sp_array.phys, capped);
            self.scratchpad_array = Some(sp_array);
        }

        unsafe { mmio_write32(self.op_base, 0x30, dcbaa.phys) };
        unsafe { mmio_write32(self.op_base, 0x34, 0) };

        let cmd_ring: Dma<[Trb; 16]> = Dma::new_zeroed(64);
        unsafe { 
            // FIX: Add a Link TRB to the end of the Command Ring to wrap execution
            let ring_ptr = cmd_ring.virt as *mut Trb;
            let mut link_trb = Trb::new();
            link_trb.data_low = cmd_ring.phys;
            link_trb.control = (6 << 10) | 2 | 1; // Type 6 (Link), TC=1, cycle=1
            ring_ptr.add(15).write_volatile(link_trb);

            mmio_write32(self.op_base, 0x18, cmd_ring.phys | 1) 
        };
        unsafe { mmio_write32(self.op_base, 0x1C, 0) };

        let event_ring: Dma<[Trb; 16]> = Dma::new_zeroed(64);
        let mut erst: Dma<[u64; 2]> = Dma::new_zeroed(64);
        unsafe {
            (*erst.virt)[0] = event_ring.phys as u64;
            (*erst.virt)[1] = 16;
        }

        let ir_base = self.runtime_base;
        unsafe {
            mmio_write32(ir_base, 0x20,       2);   
            mmio_write32(ir_base, 0x28,       1);   
            mmio_write32(ir_base, 0x30, erst.phys); 
            mmio_write32(ir_base, 0x34,       0);   
            mmio_write32(ir_base, 0x38, event_ring.phys | 8); 
            mmio_write32(ir_base, 0x3C,       0);   
        }

        let hcs_params1 = unsafe { mmio_read32(self.cap_base, 0x04) };
        let max_slots = hcs_params1 & 0xFF;
        unsafe { mmio_write32(self.op_base, 0x38, max_slots) };

        self.dcbaa = Some(dcbaa);
        self.cmd_ring = Some(cmd_ring);
        self.event_ring = Some(event_ring);
        self.erst = Some(erst);

        serial_println!("[xhci] Data structures initialized.");

        let usbcmd = unsafe { mmio_read32(self.op_base, 0) };
        unsafe { mmio_write32(self.op_base, 0, usbcmd | 1) };
        self.wait_for_halt(false);
        self.wait_for_cnr(false); // spec requires CNR=0 before issuing commands
        serial_println!("[xhci] Controller RUNNING.");
        let usbsts = unsafe { mmio_read32(self.op_base, 4) };
        serial_println!("[xhci] USBSTS after start: {:#010x}{}", usbsts,
            if (usbsts >> 12) & 1 != 0 { " *** HCE SET — controller error!" } else { "" });

        self.monitor_ports();
    }

    pub fn monitor_ports(&mut self) {
        let hcs_params1 = unsafe { mmio_read32(self.cap_base, 0x04u64) };
        let max_ports = (hcs_params1 >> 24) & 0xFF;

        for i in 1..=max_ports {
            let portsc_off = 0x400u64 + (i as u64 - 1) * 16;
            let portsc = unsafe { mmio_read32(self.op_base, portsc_off) };

            let ccs = portsc & 1;
            let ped = (portsc >> 1) & 1;
            let speed = (portsc >> 10) & 0xF;
            let csc = (portsc >> 17) & 1; 
            let prc = (portsc >> 21) & 1; 

            if csc != 0 {
                unsafe { mmio_write32(self.op_base, portsc_off, (portsc & 0x0E00_C3F0) | (1 << 17)) };
                serial_println!("[xhci] Port {}: Connect Status Changed. CCS={}", i, ccs);

                if ccs != 0 && ped == 0 {
                    serial_println!("[xhci] Port {}: Requesting Reset...", i);
                    let portsc_new = unsafe { mmio_read32(self.op_base, portsc_off) };
                    unsafe { mmio_write32(self.op_base, portsc_off, (portsc_new & 0x0E00_C3F0) | (1 << 4)) };
                }
            } else if ccs != 0 && ped == 0 {
                // Device is connected but no CSC event — happens on real hardware when a device
                // was plugged in before the controller was reset.  Force a port reset to kick
                // enumeration.  Guard with ports_addressed so we only reset once per port.
                let port_bit = 1u32 << (i - 1);
                if self.ports_addressed & port_bit == 0 {
                    serial_println!("[xhci] Port {}: pre-connected device (CCS=1, no CSC), requesting reset...", i);
                    let portsc_new = unsafe { mmio_read32(self.op_base, portsc_off) };
                    unsafe { mmio_write32(self.op_base, portsc_off, (portsc_new & 0x0E00_C3F0) | (1 << 4)) };
                    // Mark as "reset requested" so we don't keep re-resetting
                    self.ports_addressed |= port_bit;
                }
            }

            if prc != 0 {
                unsafe { mmio_write32(self.op_base, portsc_off, (portsc & 0x0E00_C3F0) | (1 << 21)) };
                serial_println!("[xhci] Port {}: Reset Complete. Enabled: {}", i, ped != 0);

                if ped != 0 {
                    // Clear the "reset requested" sentinel so address_device can set the real bit.
                    self.ports_addressed &= !(1u32 << (i - 1));
                    if let Some(slot_id) = self.enable_slot() {
                        self.address_device(slot_id, i as u8, speed as u8);
                    }
                }
            }
            
        }
    }


    pub fn enable_slot(&mut self) -> Option<u8> {
        serial_println!("[xhci] Sending 'Enable Slot' command...");
        let mut trb = Trb::new();
        trb.control = 9 << 10; 
        self.send_command(trb);

        if let Some(event) = self.poll_event() {
            let slot_id = (event.control >> 24) as u8;
            let code = (event.status >> 24) & 0xFF;
            serial_println!("[xhci] 'Enable Slot' Result: Slot={}, Code={}", slot_id, code);
            if code == 1 { Some(slot_id) } else { None }
        } else { 
            serial_println!("[xhci] 'Enable Slot' TIMEOUT");
            None 
        }
    }

    pub fn address_device(&mut self, slot_id: u8, port_id: u8, speed: u8) {
        crate::serial_println!("[xhci] Slot {} -> Port {} (Speed: {}, CSZ: {})", slot_id, port_id, speed, self.context_size);

        let device_ctx: Dma<[u32; 512]> = Dma::new_zeroed(64);
        let device_ctx_phys = device_ctx.phys;

        if let Some(dcbaa) = &mut self.dcbaa {
            unsafe {
                core::ptr::write_volatile(
                    (dcbaa.virt as *mut u64).add(slot_id as usize),
                    device_ctx_phys as u64
                );
            }
        }
        self.device_contexts[slot_id as usize] = Some(device_ctx);

        let input_context: Dma<[u32; 1024]> = Dma::new_zeroed(64);
        
        let max_packet_size = match speed {
            4 => 512,
            3 => 64,
            2 => 8,
            _ => 64,
        };

        let ep0_ring: Dma<[Trb; 16]> = Dma::new_zeroed(64);
        let ep0_ring_phys = ep0_ring.phys;

        // Use volatile writes for the context to ensure the compiler emits them exactly here
        unsafe {
            let ptr = input_context.virt as *mut u32;

            // Input Control Context
            ptr.add(self.ctx_offset(0) + 1).write_volatile(0x3);

            // Slot Context
            let slot_off = self.ctx_offset(1);
            ptr.add(slot_off + 0).write_volatile((1 << 27) | ((speed as u32) << 20));
            ptr.add(slot_off + 1).write_volatile((port_id as u32) << 16);

            // EP0 Context
            let ep0_off = self.ctx_offset(2);
            ptr.add(ep0_off + 1).write_volatile((3 << 1) | (4 << 3) | (max_packet_size << 16));
            ptr.add(ep0_off + 2).write_volatile(ep0_ring_phys | 1);
            ptr.add(ep0_off + 4).write_volatile(8);
        }

        self.ep0_rings[slot_id as usize] = Some(ep0_ring);

        let mut trb = Trb::new();
        trb.data_low = input_context.phys;
        trb.control = (11 << 10) | ((slot_id as u32) << 24);
        self.send_command(trb);

        if let Some(event) = self.poll_event() {
            let code = (event.status >> 24) & 0xFF;
            crate::serial_println!("[xhci] 'Address Device' code: {}", code);
            if code == 1 {
                self.ports_addressed |= 1 << port_id;
                self.get_device_descriptor(slot_id);
            } else {
                crate::serial_println!("[xhci] Address Device failed with code {}", code);
            }
        } else {
            crate::serial_println!("[xhci] 'Address Device' TIMEOUT");
        }
    }

    pub fn get_device_descriptor(&mut self, slot_id: u8) {
        crate::serial_println!("[xhci] GET_DESCRIPTOR (Device) slot {}", slot_id);

        let setup_data: u64 = 0x0012_0000_0100_0680;
        let mut setup_trb = Trb::new();
        setup_trb.data_low  = setup_data as u32;
        setup_trb.data_high = (setup_data >> 32) as u32;
        setup_trb.status    = 8;
        setup_trb.control   = (2 << 10) | (3 << 16) | (1 << 6) | 1; 

        let data_buf: Dma<[u8; 18]> = Dma::new_zeroed(64);
        let mut data_trb = Trb::new();
        data_trb.data_low = data_buf.phys;
        data_trb.status   = 18;
        data_trb.control  = (3 << 10) | (1 << 16) | 1; 

        let mut status_trb = Trb::new();
        // Type 4 (Status), DIR=0 (OUT for IN Data), IOC=1 (bit 5), C=1
        status_trb.control = (4 << 10) | (0 << 16) | (1 << 5) | 1; 

        if let Some(ep0_ring) = &self.ep0_rings[slot_id as usize] {
            unsafe {
                let ptr = ep0_ring.virt as *mut Trb;
                ptr.add(0).write_volatile(setup_trb);
                ptr.add(1).write_volatile(data_trb);
                ptr.add(2).write_volatile(status_trb);
                
                core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::Release);
                core::arch::asm!("sfence", options(nostack, preserves_flags));
            }
        }

        unsafe {
            let db_ptr = (self.db_base.as_u64() + (slot_id as u64) * 4) as *mut u32;
            db_ptr.write_volatile(1);
        }

        if let Some(_event) = self.poll_event() {
            crate::serial_println!("[xhci] Device Descriptor: {:02x} {:02x} {:02x}",
                unsafe { (*data_buf.virt)[0] }, unsafe { (*data_buf.virt)[1] }, unsafe { (*data_buf.virt)[2] });
            self.get_config_descriptor(slot_id);
        } else {
            crate::serial_println!("[xhci] get_device_descriptor TIMEOUT");
        }
    }

    pub fn get_config_descriptor(&mut self, slot_id: u8) {
        crate::serial_println!("[xhci] GET_DESCRIPTOR (Config) slot {}", slot_id);

        let setup_data: u64 = 0x0100_0000_0200_0680;
        let mut setup_trb = Trb::new();
        setup_trb.data_low  = setup_data as u32;
        setup_trb.data_high = (setup_data >> 32) as u32;
        setup_trb.status    = 8;
        setup_trb.control   = (2 << 10) | (3 << 16) | (1 << 6) | 1;

        let data_buf: Dma<[u8; 256]> = Dma::new_zeroed(64);
        let mut data_trb = Trb::new();
        data_trb.data_low = data_buf.phys;
        data_trb.status   = 256;
        data_trb.control  = (3 << 10) | (1 << 16) | 1;

        let mut status_trb = Trb::new();
        // Type 4 (Status), DIR=0, IOC=1, C=1
        status_trb.control = (4 << 10) | (0 << 16) | (1 << 5) | 1;

        if let Some(ep0_ring) = &self.ep0_rings[slot_id as usize] {
            unsafe {
                let ptr = ep0_ring.virt as *mut Trb;
                // Move indices to 3, 4, 5
                ptr.add(3).write_volatile(setup_trb);
                ptr.add(4).write_volatile(data_trb);
                ptr.add(5).write_volatile(status_trb);

                core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::Release);
                core::arch::asm!("sfence", options(nostack, preserves_flags));
            }
        }

        unsafe {
            let db_ptr = (self.db_base.as_u64() + (slot_id as u64) * 4) as *mut u32;
            db_ptr.write_volatile(1);
        }

        if self.poll_event().is_some() {
            self.parse_config_descriptor(slot_id, &data_buf);
        } else {
            crate::serial_println!("[xhci] get_config_descriptor TIMEOUT");
        }
    }

    fn parse_config_descriptor(&mut self, slot_id: u8, buffer: &Dma<[u8; 256]>) {
        let total_length = unsafe {
            (*buffer.virt)[2] as u16 | ((*buffer.virt)[3] as u16) << 8
        };
        crate::serial_println!("[xhci] Config Descriptor total length: {}", total_length);

        let mut offset = 0usize;
        let limit = (total_length as usize).min(256);
        let mut current_kind: Option<DeviceKind> = None;
        while offset < limit {
            let length = unsafe { (*buffer.virt)[offset] };
            if length == 0 { break; }
            let dtype = unsafe { (*buffer.virt)[offset + 1] };

            match dtype {
                4 => { // Interface
                    let class = unsafe { (*buffer.virt)[offset + 5] };
                    let subclass = unsafe { (*buffer.virt)[offset + 6] };
                    let protocol = unsafe { (*buffer.virt)[offset + 7] };

                    if class == 3 {
                        let (dev_type, kind) = match protocol {
                            1 => ("Keyboard",    Some(DeviceKind::Keyboard)),
                            2 => ("Mouse/Tablet", Some(DeviceKind::Mouse)),
                            _ => ("Unknown",     None),
                        };
                        crate::serial_println!("[xhci] HID Interface ({}) found at slot {} (Subclass: {}, Protocol: {})",
                            dev_type, slot_id, subclass, protocol);
                        current_kind = kind;
                    } else {
                        current_kind = None;
                    }
                }
                5 => { // Endpoint
                    let ep_addr   = unsafe { (*buffer.virt)[offset + 2] };
                    let attrs     = unsafe { (*buffer.virt)[offset + 3] };
                    if (attrs & 0x3) == 0x3 { // Interrupt
                        crate::serial_println!("[xhci] Interrupt EP 0x{:02x} at slot {}", ep_addr, slot_id);
                        if (ep_addr & 0x80) != 0 {
                            if let Some(kind) = current_kind {
                                let max_packet_size = unsafe {
                                    (*buffer.virt)[offset + 4] as u16 | ((*buffer.virt)[offset + 5] as u16) << 8
                                };
                                let interval = unsafe { (*buffer.virt)[offset + 6] };
                                self.configure_hid_endpoint(slot_id, ep_addr, max_packet_size, interval, kind);
                            }
                        }
                    }
                }
                _ => {}
            }

            offset += length as usize;
        }
    }

    fn send_command(&mut self, mut trb: Trb) {
        if let Some(ring) = &mut self.cmd_ring {
            let idx = self.cmd_idx;
            let cycle = self.cmd_cycle;

            trb.control |= cycle; 
            
            unsafe {
                // Break the TRB down into 32-bit writes to prevent tearing
                let ring_ptr = ring.virt as *mut u32;
                let trb_ptr = ring_ptr.add(idx * 4); 

                // 1. Write the first 12 bytes (Data + Status)
                trb_ptr.add(0).write_volatile(trb.data_low);
                trb_ptr.add(1).write_volatile(trb.data_high);
                trb_ptr.add(2).write_volatile(trb.status);

                // 2. Hardware fence: Force CPU to push to RAM before the cycle bit flips
                core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::Release);
                core::arch::asm!("sfence", options(nostack, preserves_flags));

                // 3. Write the control word (flips the Cycle Bit)
                trb_ptr.add(3).write_volatile(trb.control);

                // 4. Hardware fence: Force TRB to RAM before the Doorbell MMIO write
                core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::Release);
                core::arch::asm!("sfence", options(nostack, preserves_flags));
            }

            self.cmd_idx += 1;
            if self.cmd_idx >= 15 {
                self.cmd_idx = 0;
                self.cmd_cycle ^= 1;
            }

            // Ring the doorbell
            unsafe {
                (self.db_base.as_u64() as *mut u32).write_volatile(0); 
            }
        }
    }

    pub fn poll_event(&mut self) -> Option<Trb> {
        if let Some(event) = self.pending_command_completion.take() {
            return Some(event);
        }
        if let Some(event) = self.pending_transfer_completion.take() {
            return Some(event);
        }

        let deadline = crate::interrupts::TICKS.load(Ordering::Relaxed).wrapping_add(50);
        // Spin-count fallback: guarantees exit even if TICKS is not advancing
        // (e.g. during init before interrupts are enabled).
        // 50M iterations ≈ 50–500 ms on modern CPUs — real hardware xHCI
        // commands can take significantly longer than QEMU's virtual controller.
        let mut spins: u64 = 0;
        loop {
            if crate::interrupts::TICKS.load(Ordering::Relaxed) >= deadline {
                break;
            }
            if spins >= 50_000_000 {
                break;
            }
            spins += 1;
            if let Some(event) = self.try_event() {
                self.handle_event(event);

                if let Some(ev) = self.pending_command_completion.take() {
                    return Some(ev);
                }
                if let Some(ev) = self.pending_transfer_completion.take() {
                    return Some(ev);
                }
            }
            core::hint::spin_loop();
        }
        crate::serial_println!("[xhci] poll_event TIMEOUT");
        None
    }

    pub fn try_event(&mut self) -> Option<Trb> {
        let idx = self.event_idx;
        let cycle = self.event_cycle;

        if let Some(ring) = &mut self.event_ring {
            unsafe {
                // Point directly to the exact TRB in memory
                let trb_ptr = (ring.virt as *const Trb).add(idx);
                
                // 1. Force a volatile read of the Control word to bypass cache
                let control = core::ptr::read_volatile(&((*trb_ptr).control));
                
                // 2. Check if the cycle bit matches our software state
                if (control & 1) == cycle {
                    // The hardware flipped the bit! Read the rest of the data safely.
                    let mut event = Trb::new();
                    event.data_low = core::ptr::read_volatile(&((*trb_ptr).data_low));
                    event.data_high = core::ptr::read_volatile(&((*trb_ptr).data_high));
                    event.status = core::ptr::read_volatile(&((*trb_ptr).status));
                    event.control = control;
                    
                    // Advance our pointer
                    self.event_idx += 1;
                    if self.event_idx >= 16 {
                        self.event_idx = 0;
                        self.event_cycle ^= 1;
                    }

                    // Tell the hardware we consumed the event
                    let new_idx = self.event_idx;
                    let phys_ptr = ring.phys as u64 + (new_idx as u64 * 16);
                    mmio_write32(self.runtime_base, 0x38, (phys_ptr as u32) | 8);
                    mmio_write32(self.runtime_base, 0x3C, (phys_ptr >> 32) as u32);
                    
                    return Some(event);
                }
            }
        }
        None
    }

    pub fn handle_event(&mut self, event: Trb) {
        let trb_type = (event.control >> 10) & 0x3F;
        
        match trb_type {
            34 => { // Port Status Change
                self.needs_monitor = true;
            }
            32 => { // Transfer
                let slot_id = (event.control >> 24) as u8;
                let ep_id = (event.control >> 16) & 0x1F;
                let comp_code = (event.status >> 24) & 0xFF;
                
                // EP 1 is the Control Endpoint (EP0)
                if ep_id == 1 {
                    self.pending_transfer_completion = Some(event);
                } else {
                    for hid in self.hid_devices.iter_mut() {
                        if hid.slot_id == slot_id && hid.ep_index == ep_id as u8 {
                            // Code 1 = Success, Code 13 = Short Packet (Very common for HID)
                            if comp_code == 1 || comp_code == 13 {
                                handle_hid_report(hid);
                            } else {
                                crate::serial_println!("[xhci] Transfer failed with code {}", comp_code);
                            }
                            // Unblock the endpoint so poll_usb_devices can queue the next read
                            hid.pending = false; 
                        }
                    }
                }
            }
            33 => { // Command Completion
                self.pending_command_completion = Some(event);
            }
            _ => {}
        }
    }
}

lazy_static! {
    pub static ref CONTROLLERS: Mutex<Vec<XhciController>> = Mutex::new(Vec::new());
}

pub fn init(
    mapper: &mut impl x86_64::structures::paging::Mapper<x86_64::structures::paging::Size4KiB>,
    frame_allocator: &mut impl x86_64::structures::paging::FrameAllocator<x86_64::structures::paging::Size4KiB>,
) {
    use x86_64::structures::paging::{Page, PageTableFlags, PhysFrame};
    use x86_64::{PhysAddr, VirtAddr};

    serial_println!("[xhci] find_xhci_controllers...");
    let controllers = crate::pci::find_xhci_controllers();
    serial_println!("[xhci] found {} controller(s)", controllers.len());
    let mut list = CONTROLLERS.lock();
    for dev in controllers {
        // The xHCI BAR is MMIO — not RAM — so it is not covered by the
        // bootloader's physical-memory offset mapping on UEFI systems.
        // Map 256 KiB (64 pages) at phys_offset + bar_base before any
        // MMIO access occurs inside XhciController::new().
        let bar0 = dev.read_bar(0);
        let base_phys = if (bar0 & 0x6) == 0x4 {
            let bar1 = dev.read_bar(1);
            ((bar1 as u64) << 32) | (bar0 as u64 & !0xF)
        } else {
            bar0 as u64 & !0xF
        };
        let phys_offset = PHYS_MEM_OFFSET.load(Ordering::Relaxed);
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_CACHE;
        for page_idx in 0u64..64 {
            let phys = PhysAddr::new(base_phys + page_idx * 0x1000);
            let virt = VirtAddr::new(base_phys + phys_offset + page_idx * 0x1000);
            let frame = PhysFrame::containing_address(phys);
            let page = Page::containing_address(virt);
            unsafe {
                match mapper.map_to(page, frame, flags, frame_allocator) {
                    Ok(flush) => flush.flush(),
                    Err(_) => {} // already mapped (QEMU BIOS mode)
                }
            }
        }
        serial_println!("[xhci] BAR MMIO mapped, creating controller...");

        let mut xhci = XhciController::new(dev);
        serial_println!("[xhci] calling xhci.init()...");
        xhci.init();
        list.push(xhci);
    }
}

pub fn poll_usb_devices() {
    if let Some(mut list) = CONTROLLERS.try_lock() {
        for xhci in list.iter_mut() {
            while let Some(event) = xhci.try_event() {
                xhci.handle_event(event);
            }

            if xhci.needs_monitor {
                xhci.needs_monitor = false;
                xhci.monitor_ports();
            }

            // Key repeat: fire synthetic key events for held keyboard keys
            let now = crate::interrupts::TICKS.load(Ordering::Relaxed);
            let hid_count = xhci.hid_devices.len();
            for i in 0..hid_count {
                if xhci.hid_devices[i].kind == DeviceKind::Keyboard {
                    let key = xhci.hid_devices[i].repeat_key;
                    if key != 0 && now >= xhci.hid_devices[i].repeat_next_tick {
                        if let Some((scancode, extended)) = hid_to_ps2(key) {
                            push_key(scancode, extended, false);
                        }
                        xhci.hid_devices[i].repeat_next_tick = now + REPEAT_INTERVAL_TICKS;
                    }
                }
            }

            for i in 0..hid_count {
                if !xhci.hid_devices[i].pending {
                    let slot_id = xhci.hid_devices[i].slot_id;
                    let ep_idx = xhci.hid_devices[i].ep_index;
                    let buf_phys = xhci.hid_devices[i].buffer.phys;
                    
                    let trb_idx = xhci.hid_devices[i].ring_idx;
                    let cycle = xhci.hid_devices[i].ring_cycle;
                    
                    unsafe {
                        let ring_ptr = xhci.hid_devices[i].ring.virt as *mut u32;
                        let trb_ptr = ring_ptr.add(trb_idx * 4);

                        // 1. Write Data and Status
                        trb_ptr.add(0).write_volatile(buf_phys);
                        trb_ptr.add(1).write_volatile(0);
                        trb_ptr.add(2).write_volatile(8); // Length of HID report

                        // 2. Fence before cycle bit
                        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::Release);
                        core::arch::asm!("sfence", options(nostack, preserves_flags));

                        // 3. Write Control (Type 1, IOC=1, plus cycle bit)
                        let control = (1 << 10) | (1 << 5) | cycle;
                        trb_ptr.add(3).write_volatile(control);

                        // 4. Fence before Doorbell or Link TRB
                        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::Release);
                        core::arch::asm!("sfence", options(nostack, preserves_flags));
                    }

                    if trb_idx == 14 {
                        let ring_phys = xhci.hid_devices[i].ring.phys;
                        
                        unsafe {
                            let ring_ptr = xhci.hid_devices[i].ring.virt as *mut u32;
                            let link_ptr = ring_ptr.add(15 * 4);
                            
                            link_ptr.add(0).write_volatile(ring_phys);
                            link_ptr.add(1).write_volatile(0);
                            link_ptr.add(2).write_volatile(0);
                            
                            core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::Release);
                            core::arch::asm!("sfence", options(nostack, preserves_flags));
                            
                            // Type 6 (Link), TC=1, plus cycle bit
                            let link_control = (6 << 10) | 2 | cycle;
                            link_ptr.add(3).write_volatile(link_control);

                            core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::Release);
                            core::arch::asm!("sfence", options(nostack, preserves_flags));
                        }
                        
                        xhci.hid_devices[i].ring_idx = 0;
                        xhci.hid_devices[i].ring_cycle ^= 1;
                    } else {
                        xhci.hid_devices[i].ring_idx += 1;
                    }

                    unsafe {
                        let db_ptr = (xhci.db_base.as_u64() + (slot_id as u64) * 4) as *mut u32;
                        db_ptr.write_volatile(ep_idx as u32);
                    }
                    xhci.hid_devices[i].pending = true;
                }
            }
        }
    }
}

fn handle_hid_report(hid: &mut HidDevice) {
    // Read the 8-byte HID report from physical RAM using volatile reads
    let mut report = [0u8; 8];
    unsafe {
        let ptr = hid.buffer.virt as *const u8;
        for i in 0..8 {
            report[i] = core::ptr::read_volatile(ptr.add(i));
        }
    }

    match hid.kind {
        DeviceKind::Keyboard => handle_keyboard_report(hid, &report),
        DeviceKind::Mouse    => handle_mouse_report(&report),
    }

    hid.prev_report.copy_from_slice(&report);
}

fn push_key(scancode: u8, extended: bool, release: bool) {
    if extended {
        let _ = crate::keyboard::push_scancode(0xE0);
    }
    let _ = crate::keyboard::push_scancode(if release { scancode | 0x80 } else { scancode });
}

// Initial delay before repeat starts: ~500ms at 18Hz
const REPEAT_DELAY_TICKS: u64 = 9;
// Interval between repeats: ~110ms at 18Hz (~9 repeats/sec)
const REPEAT_INTERVAL_TICKS: u64 = 2;

fn handle_keyboard_report(hid: &mut HidDevice, report: &[u8; 8]) {
    let modifiers      = report[0];
    let prev_modifiers = hid.prev_report[0];

    // Modifier keys: (HID bit, PS/2 scancode, extended)
    let modifier_map: &[(u8, u8, bool)] = &[
        (0x01, 0x1D, false), // L-Ctrl
        (0x02, 0x2A, false), // L-Shift
        (0x04, 0x38, false), // L-Alt
        (0x08, 0x5B, true),  // L-GUI (Windows key)
        (0x10, 0x1D, true),  // R-Ctrl
        (0x20, 0x36, false), // R-Shift
        (0x40, 0x38, true),  // R-Alt
        (0x80, 0x5C, true),  // R-GUI
    ];

    for (bit, scancode, extended) in modifier_map.iter() {
        if (modifiers & bit) != 0 && (prev_modifiers & bit) == 0 {
            push_key(*scancode, *extended, false);
        } else if (modifiers & bit) == 0 && (prev_modifiers & bit) != 0 {
            push_key(*scancode, *extended, true);
        }
    }

    // Key-down events
    for i in 2..8 {
        let key = report[i];
        if key != 0 && !hid.prev_report[2..8].contains(&key) {
            if let Some((scancode, extended)) = hid_to_ps2(key) {
                push_key(scancode, extended, false);
            }
        }
    }

    // Key-up events
    for i in 2..8 {
        let key = hid.prev_report[i];
        if key != 0 && !report[2..8].contains(&key) {
            if let Some((scancode, extended)) = hid_to_ps2(key) {
                push_key(scancode, extended, true);
            }
        }
    }

    // Update repeat state: track the single held key (if exactly one is down)
    let held: u8 = report[2..8].iter().copied().find(|&k| k != 0).unwrap_or(0);
    let held_count = report[2..8].iter().filter(|&&k| k != 0).count();
    if held_count == 1 && held != hid.repeat_key {
        // New single key held — start repeat delay
        hid.repeat_key = held;
        hid.repeat_next_tick =
            crate::interrupts::TICKS.load(Ordering::Relaxed) + REPEAT_DELAY_TICKS;
    } else if held_count != 1 {
        hid.repeat_key = 0;
    }
}

fn handle_mouse_report(report: &[u8; 8]) {
    use core::sync::atomic::Ordering;

    // HID boot protocol mouse report:
    //   Byte 0: button bitmask (bit 0=left, bit 1=right, bit 2=middle)
    //   Byte 1: X delta (signed i8)
    //   Byte 2: Y delta (signed i8, HID Y+ is down, same as PS/2)
    //   Byte 3: scroll wheel delta (signed i8, present on most USB mice
    //           even in boot protocol mode; positive = scroll down)
    let buttons = report[0] & 0x07;
    let dx      = report[1] as i8 as i32;
    let dy      = report[2] as i8 as i32;
    let scroll  = report[3] as i8 as i32;

    crate::mouse::MOUSE_BTN.store(buttons, Ordering::Relaxed);

    let (max_w, max_h) = crate::framebuffer::get_resolution();
    let new_x = (crate::mouse::MOUSE_X.load(Ordering::Relaxed) + dx)
        .clamp(0, max_w as i32 - 1);
    // HID Y+ is down; framebuffer Y+ is also down — no sign flip needed
    let new_y = (crate::mouse::MOUSE_Y.load(Ordering::Relaxed) + dy)
        .clamp(0, max_h as i32 - 1);
    crate::mouse::MOUSE_X.store(new_x, Ordering::Relaxed);
    crate::mouse::MOUSE_Y.store(new_y, Ordering::Relaxed);

    if scroll != 0 {
        crate::mouse::MOUSE_SCROLL.fetch_add(scroll, Ordering::Relaxed);
    }
}

/// Returns (ps2_scancode, is_extended).
fn hid_to_ps2(k: u8) -> Option<(u8, bool)> {
    let s = |sc| Some((sc, false));
    let e = |sc| Some((sc, true));
    match k {
        // Letters — PS/2 Set 1 positions follow physical layout, not alphabet order
        0x04 => s(0x1E), // A
        0x05 => s(0x30), // B
        0x06 => s(0x2E), // C
        0x07 => s(0x20), // D
        0x08 => s(0x12), // E
        0x09 => s(0x21), // F
        0x0A => s(0x22), // G
        0x0B => s(0x23), // H
        0x0C => s(0x17), // I
        0x0D => s(0x24), // J
        0x0E => s(0x25), // K
        0x0F => s(0x26), // L
        0x10 => s(0x32), // M
        0x11 => s(0x31), // N
        0x12 => s(0x18), // O
        0x13 => s(0x19), // P
        0x14 => s(0x10), // Q
        0x15 => s(0x13), // R
        0x16 => s(0x1F), // S
        0x17 => s(0x14), // T
        0x18 => s(0x16), // U
        0x19 => s(0x2F), // V
        0x1A => s(0x11), // W
        0x1B => s(0x2D), // X
        0x1C => s(0x15), // Y
        0x1D => s(0x2C), // Z
        // Number row
        0x1E => s(0x02), // 1
        0x1F => s(0x03), // 2
        0x20 => s(0x04), // 3
        0x21 => s(0x05), // 4
        0x22 => s(0x06), // 5
        0x23 => s(0x07), // 6
        0x24 => s(0x08), // 7
        0x25 => s(0x09), // 8
        0x26 => s(0x0A), // 9
        0x27 => s(0x0B), // 0
        // Control keys
        0x28 => s(0x1C), // Enter
        0x29 => s(0x01), // Escape
        0x2A => s(0x0E), // Backspace
        0x2B => s(0x0F), // Tab
        0x2C => s(0x39), // Space
        // Punctuation
        0x2D => s(0x0C), // -
        0x2E => s(0x0D), // =
        0x2F => s(0x1A), // [
        0x30 => s(0x1B), // ]
        0x31 => s(0x2B), // backslash
        0x32 => s(0x2B), // # (non-US)
        0x33 => s(0x27), // ;
        0x34 => s(0x28), // '
        0x35 => s(0x29), // `
        0x36 => s(0x33), // ,
        0x37 => s(0x34), // .
        0x38 => s(0x35), // /
        // Function keys
        0x3A => s(0x3B), // F1
        0x3B => s(0x3C), // F2
        0x3C => s(0x3D), // F3
        0x3D => s(0x3E), // F4
        0x3E => s(0x3F), // F5
        0x3F => s(0x40), // F6
        0x40 => s(0x41), // F7
        0x41 => s(0x42), // F8
        0x42 => s(0x43), // F9
        0x43 => s(0x44), // F10
        0x44 => s(0x57), // F11
        0x45 => s(0x58), // F12
        // Navigation — extended keys
        0x49 => e(0x52), // Insert
        0x4A => e(0x47), // Home
        0x4B => e(0x49), // Page Up
        0x4C => e(0x53), // Delete
        0x4D => e(0x4F), // End
        0x4E => e(0x51), // Page Down
        0x4F => e(0x4D), // Right arrow
        0x50 => e(0x4B), // Left arrow
        0x51 => e(0x50), // Down arrow
        0x52 => e(0x48), // Up arrow
        _ => None,
    }
}