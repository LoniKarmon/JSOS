// src/storage.rs — JSKV Persistent Object Store & FAT
// Uses ata.rs to read/write JSON/string blobs seamlessly.

use crate::ata::PRIMARY_DRIVE;
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::mem::size_of;
use lazy_static::lazy_static;
use spin::Mutex;

const MAGIC: [u8; 4] = *b"JSKV";
const BLOCK_SIZE: usize = 512;
const FAT_START_LBA: u32 = 2;

// Support 64MB disk = 131072 blocks
// FAT size = 131072 * 4 bytes = 512 KB = 1024 sectors
const TOTAL_BLOCKS: u32 = 131072; 
const FAT_SECTORS: u32 = (TOTAL_BLOCKS * 4) / (BLOCK_SIZE as u32);
const MFT_START_LBA: u32 = FAT_START_LBA + FAT_SECTORS;
// Support 128 keys in MFT = 128 * 128 bytes = 16384 bytes = 32 sectors
const MFT_SECTORS: u32 = 32;
const DATA_START_LBA: u32 = MFT_START_LBA + MFT_SECTORS;

const FAT_FREE: u32 = 0;
const FAT_EOF: u32 = 0xFFFFFFFF;

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Superblock {
    magic: [u8; 4],
    key_count: u32,
    total_blocks: u32,
    reserved: [u8; 500],
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct MftEntry {
    key: [u8; 116],
    start_lba: u32,
    size_bytes: u32,
    flags: u32, // 1 = Valid, 0 = Deleted
}

const MFT_ENTRY_SIZE: usize = size_of::<MftEntry>();
const ENTRIES_PER_SECTOR: usize = BLOCK_SIZE / MFT_ENTRY_SIZE;
const MAX_KEYS: usize = (MFT_SECTORS as usize) * ENTRIES_PER_SECTOR;

lazy_static! {
    // In-memory FAT cache
    static ref FAT: Mutex<Vec<u32>> = Mutex::new(alloc::vec![0; TOTAL_BLOCKS as usize]);
    
    // In-memory MFT cache
    static ref MFT: Mutex<BTreeMap<String, MftEntry>> = Mutex::new(BTreeMap::new());
    
    static ref INITIALIZED: Mutex<bool> = Mutex::new(false);
}

pub fn init() {
    let mut drive = PRIMARY_DRIVE.lock();
    let mut sb_buf = [0u8; 512];
    
    if !drive.read_sector(1, &mut sb_buf) {
        crate::serial_println!("[Storage] Failed to read Superblock.");
        return;
    }
    
    let magic = &sb_buf[0..4];
    if magic != MAGIC {
        crate::serial_println!("[Storage] JSKV not found. Formatting disk...");
        format_disk(&mut drive);
    } else {
        crate::serial_println!("[Storage] JSKV found. Loading FAT and MFT...");
        crate::serial_println!("[Storage] load_fat start");
        load_fat(&mut drive);
        crate::serial_println!("[Storage] load_fat done");
        load_mft(&mut drive);
        crate::serial_println!("[Storage] load_mft done");
    }

    crate::serial_println!("[Storage] setting INITIALIZED");
    *INITIALIZED.lock() = true;
    crate::serial_println!("[Storage] init complete");
}

fn format_disk(drive: &mut crate::ata::AtaDrive) {
    // 1. Write Superblock
    let mut sb = [0u8; 512];
    sb[0..4].copy_from_slice(&MAGIC);
    let total_blocks_bytes = TOTAL_BLOCKS.to_le_bytes();
    sb[8..12].copy_from_slice(&total_blocks_bytes);
    drive.write_sector(1, &sb);
    
    // 2. Clear FAT (all 0s)
    let empty_fat = [0u8; 512];
    for i in 0..FAT_SECTORS {
        drive.write_sector(FAT_START_LBA + i, &empty_fat);
    }
    
    // 3. Clear MFT (all 0s)
    let empty_mft = [0u8; 512];
    for i in 0..MFT_SECTORS {
        drive.write_sector(MFT_START_LBA + i, &empty_mft);
    }
    
    crate::serial_println!("[Storage] Format complete.");
}

fn load_fat(drive: &mut crate::ata::AtaDrive) {
    crate::serial_println!("[Storage] load_fat: locking FAT");
    let mut fat = FAT.lock();
    crate::serial_println!("[Storage] load_fat: FAT locked, reading {} sectors", FAT_SECTORS);
    // Read the FAT sectors into our u32 vector
    for i in 0..FAT_SECTORS {
        let mut buf = [0u8; 512];
        if drive.read_sector(FAT_START_LBA + i, &mut buf) {
            let offset = (i as usize) * 128; // 128 u32s per sector
            for j in 0..128 {
                let val = u32::from_le_bytes([
                    buf[j*4], buf[j*4+1], buf[j*4+2], buf[j*4+3]
                ]);
                fat[offset + j] = val;
            }
        }
    }
}

fn load_mft(drive: &mut crate::ata::AtaDrive) {
    let mut mft = MFT.lock();
    mft.clear();
    
    for i in 0..MFT_SECTORS {
        let mut buf = [0u8; 512];
        if drive.read_sector(MFT_START_LBA + i, &mut buf) {
            for j in 0..ENTRIES_PER_SECTOR {
                let offset = j * MFT_ENTRY_SIZE;
                let flags_bytes = [
                    buf[offset + 124], buf[offset + 125], buf[offset + 126], buf[offset + 127]
                ];
                let flags = u32::from_le_bytes(flags_bytes);
                
                if flags == 1 {
                    let mut key_end = 116;
                    for k in 0..116 {
                        if buf[offset + k] == 0 {
                            key_end = k;
                            break;
                        }
                    }
                    if let Ok(key_str) = core::str::from_utf8(&buf[offset..offset+key_end]) {
                        let start_lba = u32::from_le_bytes([
                            buf[offset + 116], buf[offset + 117], buf[offset + 118], buf[offset + 119]
                        ]);
                        let size_bytes = u32::from_le_bytes([
                            buf[offset + 120], buf[offset + 121], buf[offset + 122], buf[offset + 123]
                        ]);
                        
                        let mut key_buf = [0u8; 116];
                        key_buf[..116].copy_from_slice(&buf[offset..offset+116]);
                        
                        mft.insert(key_str.to_string(), MftEntry {
                            key: key_buf,
                            start_lba,
                            size_bytes,
                            flags: 1,
                        });
                    }
                }
            }
        }
    }
}

fn allocate_blocks(count: u32) -> Option<Vec<u32>> {
    let mut fat = FAT.lock();
    let mut allocated = Vec::new();
    
    // We only allocate from DATA_START_LBA onwards
    for i in DATA_START_LBA..TOTAL_BLOCKS {
        if fat[i as usize] == FAT_FREE {
            allocated.push(i);
            if allocated.len() as u32 == count {
                break;
            }
        }
    }
    
    if allocated.len() as u32 != count {
        return None; // Not enough space
    }
    
    // Link them
    for i in 0..count-1 {
        fat[allocated[i as usize] as usize] = allocated[(i + 1) as usize];
    }
    fat[allocated[(count - 1) as usize] as usize] = FAT_EOF;
    
    // Flush modified FAT sectors
    let mut drive = PRIMARY_DRIVE.lock();
    let mut dirty_sectors = alloc::collections::BTreeSet::new();
    for &lba in &allocated {
        let fat_idx = lba as usize;
        let sector = (fat_idx * 4) / BLOCK_SIZE;
        dirty_sectors.insert(sector as u32);
    }
    
    for &sector in &dirty_sectors {
        let mut buf = [0u8; 512];
        let offset = (sector as usize) * 128;
        for j in 0..128 {
            let val = fat[offset + j];
            buf[j*4..j*4+4].copy_from_slice(&val.to_le_bytes());
        }
        drive.write_sector(FAT_START_LBA + sector, &buf);
    }
    
    Some(allocated)
}

fn free_chain(start_lba: u32) {
    if start_lba == 0 { return; }
    
    let mut fat = FAT.lock();
    let mut curr = start_lba;
    
    let mut dirty_sectors = alloc::collections::BTreeSet::new();
    
    while curr != FAT_EOF && curr < TOTAL_BLOCKS {
        let next = fat[curr as usize];
        fat[curr as usize] = FAT_FREE;
        
        let sector = (curr as usize * 4) / BLOCK_SIZE;
        dirty_sectors.insert(sector as u32);
        
        curr = next;
    }
    
    let mut drive = PRIMARY_DRIVE.lock();
    for &sector in &dirty_sectors {
        let mut buf = [0u8; 512];
        let offset = (sector as usize) * 128;
        for j in 0..128 {
            let val = fat[offset + j];
            buf[j*4..j*4+4].copy_from_slice(&val.to_le_bytes());
        }
        drive.write_sector(FAT_START_LBA + sector, &buf);
    }
}

pub fn write_object(key: &str, data: &[u8]) -> bool {
    if !*INITIALIZED.lock() { return false; }
    if key.len() > 115 { return false; } // Must fit and be null terminated
    
    let blocks_needed = ((data.len() + BLOCK_SIZE - 1) / BLOCK_SIZE) as u32;
    let blocks_needed = if blocks_needed == 0 { 1 } else { blocks_needed };
    
    // Check if it already exists, and free the old chain
    if let Some(old_entry) = MFT.lock().get(key) {
        free_chain(old_entry.start_lba);
    }
    
    // Allocate new chain
    let chain = match allocate_blocks(blocks_needed) {
        Some(c) => c,
        None => return false, // Disk full
    };
    
    let start_lba = chain[0];
    let mut drive = PRIMARY_DRIVE.lock();
    
    // Write data blocks
    for (i, &lba) in chain.iter().enumerate() {
        let mut buf = [0u8; 512];
        let start_idx = i * 512;
        let end_idx = core::cmp::min(start_idx + 512, data.len());
        if start_idx < data.len() {
            buf[0..(end_idx - start_idx)].copy_from_slice(&data[start_idx..end_idx]);
        }
        drive.write_sector(lba, &buf);
    }
    
    // Update MFT
    let mut mft = MFT.lock();
    let mut key_buf = [0u8; 116];
    key_buf[..key.len()].copy_from_slice(key.as_bytes());
    
    mft.insert(key.to_string(), MftEntry {
        key: key_buf,
        start_lba,
        size_bytes: data.len() as u32,
        flags: 1,
    });
    
    // Flush MFT to disk completely by rebuilding the sectors from RAM
    for sector in 0..MFT_SECTORS {
        let mut buf = [0u8; 512];
        let entries_to_skip = (sector as usize) * ENTRIES_PER_SECTOR;
        let iter = mft.values().skip(entries_to_skip).take(ENTRIES_PER_SECTOR);
        
        for (j, entry) in iter.enumerate() {
            let offset = j * MFT_ENTRY_SIZE;
            buf[offset..offset+116].copy_from_slice(&entry.key);
            buf[offset+116..offset+120].copy_from_slice(&entry.start_lba.to_le_bytes());
            buf[offset+120..offset+124].copy_from_slice(&entry.size_bytes.to_le_bytes());
            buf[offset+124..offset+128].copy_from_slice(&entry.flags.to_le_bytes());
        }
        drive.write_sector(MFT_START_LBA + sector, &buf);
    }
    
    true
}

pub fn read_object(key: &str) -> Option<Vec<u8>> {
    if !*INITIALIZED.lock() { return None; }
    
    let (start_lba, size_bytes) = {
        let mft = MFT.lock();
        if let Some(entry) = mft.get(key) {
            (entry.start_lba, entry.size_bytes)
        } else {
            return None;
        }
    };
    
    let mut fat = FAT.lock();
    let mut drive = PRIMARY_DRIVE.lock();
    
    let mut data = Vec::with_capacity(size_bytes as usize);
    let mut curr = start_lba;
    
    while curr != FAT_EOF && curr < TOTAL_BLOCKS {
        let mut buf = [0u8; 512];
        if drive.read_sector(curr, &mut buf) {
            let remaining = size_bytes as usize - data.len();
            let to_copy = core::cmp::min(remaining, 512);
            data.extend_from_slice(&buf[0..to_copy]);
        }
        
        curr = fat[curr as usize];
        if data.len() == size_bytes as usize {
            break;
        }
    }
    
    Some(data)
}

pub fn list_objects() -> Vec<String> {
    if !*INITIALIZED.lock() { return Vec::new(); }
    MFT.lock().keys().cloned().collect()
}

pub fn delete_object(key: &str) -> bool {
    if !*INITIALIZED.lock() { return false; }
    
    let start_lba = {
        let mut mft = MFT.lock();
        if let Some(entry) = mft.remove(key) {
            entry.start_lba
        } else {
            return false;
        }
    };
    
    free_chain(start_lba);
    
    // Flush MFT to disk
    let mut drive = PRIMARY_DRIVE.lock();
    let mft = MFT.lock();
    for sector in 0..MFT_SECTORS {
        let mut buf = [0u8; 512];
        let entries_to_skip = (sector as usize) * ENTRIES_PER_SECTOR;
        let iter = mft.values().skip(entries_to_skip).take(ENTRIES_PER_SECTOR);
        
        for (j, entry) in iter.enumerate() {
            let offset = j * MFT_ENTRY_SIZE;
            buf[offset..offset+116].copy_from_slice(&entry.key);
            buf[offset+116..offset+120].copy_from_slice(&entry.start_lba.to_le_bytes());
            buf[offset+120..offset+124].copy_from_slice(&entry.size_bytes.to_le_bytes());
            buf[offset+124..offset+128].copy_from_slice(&entry.flags.to_le_bytes());
        }
        drive.write_sector(MFT_START_LBA + sector, &buf);
    }
    true
}
