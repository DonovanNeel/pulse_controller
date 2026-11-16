/*
This code is adapted from my os kernel project. That project used x86_64, so a lot
needed to be changed to adapt it for AArch64.
The os kernel version was itself from Phil Oppermann's Writing an OS in Rust blog.
*/
use core::arch::asm;
use limine::memory_map::{Entry, EntryType};
use limine::response::MemoryMapResponse;

const PAGE_SIZE: u64 = 4096;
const PHYS_ADDR_MASK: u64 = 0x0000_FFFF_FFFF_F000;

// AArch64 4KB Granule Descriptor Flags
const DESC_VALID: u64 = 1 << 0;
const DESC_TABLE: u64 = 1 << 1;
const DESC_PAGE: u64 = 1 << 1;
const DESC_AF: u64 = 1 << 10;   // Access Flag (must be set to avoid AF fault)
const DESC_PXN: u64 = 1 << 53;  // Privileged Execute Never
const DESC_UXN: u64 = 1 << 54;  // Unprivileged Execute Never

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhysAddr(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtAddr(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhysFrame {
    pub start_address: PhysAddr,
}

impl PhysFrame {
    pub fn containing_address(addr: PhysAddr) -> Self {
        Self {
            start_address: PhysAddr(addr.0 & PHYS_ADDR_MASK),
        }
    }
}

/// Architecture-agnostic FrameAllocator trait
pub unsafe trait FrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame>;
}

/// Adapted BootInfoFrameAllocator using Limine's MemoryMapResponse.
pub struct BootInfoFrameAllocator {
    entries: &'static [&'static Entry],
    next: usize,
}

impl BootInfoFrameAllocator {
    /// Safety: Caller must guarantee the Limine memory map is valid and
    /// USABLE frames are not already claimed.
    pub unsafe fn init(memory_map: &'static MemoryMapResponse) -> Self {
        Self {
            entries: memory_map.entries(),
            next: 0,
        }
    }

    fn usable_frames(&self) -> impl Iterator<Item = PhysFrame> + '_ {
        self.entries
            .iter()
            .filter(|entry| entry.entry_type == EntryType::USABLE)
            .flat_map(|entry| {
                let start = entry.base;
                let end = entry.base + entry.length;
                (start..end).step_by(PAGE_SIZE as usize)
            })
            .map(|addr| PhysFrame::containing_address(PhysAddr(addr)))
    }
}

unsafe impl FrameAllocator for BootInfoFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        let frame = self.usable_frames().nth(self.next);
        self.next += 1;
        frame
    }
}

/// Represents a 512-entry AArch64 translation table (L0, L1, L2, or L3).
#[repr(C, align(4096))]
pub struct PageTable {
    entries: [u64; 512],
}

impl PageTable {
    pub fn zero(&mut self) {
        self.entries.fill(0);
    }
}

/// Adapted OffsetPageTable for AArch64 higher-half paging (TTBR1_EL1).
pub struct OffsetPageTable<'a> {
    level_0_table: &'a mut PageTable,
    physical_memory_offset: VirtAddr,
}

impl<'a> OffsetPageTable<'a> {
    /// Safety: Caller must guarantee complete physical RAM is mapped at
    /// `physical_memory_offset` (Limine HHDM) and call this only once.
    pub unsafe fn init(physical_memory_offset: VirtAddr) -> Self {
        let level_0_table = active_level_0_table(physical_memory_offset);
        Self {
            level_0_table,
            physical_memory_offset,
        }
    }

    /// Maps a 4KB physical MMIO page to a PhysAddr as uncached Device memory.
    pub unsafe fn map_mmio_page<A: FrameAllocator>(
        &mut self,
        phys_addr: PhysAddr,
        frame_allocator: &mut A,
    ) -> *mut u8 {
        let phys_aligned = phys_addr.0 & PHYS_ADDR_MASK;
        let offset_in_page = phys_addr.0 & 0xFFF;
        let virt_addr = self.physical_memory_offset.0 + phys_aligned;

        let indices = [
            ((virt_addr >> 39) & 0x1FF) as usize, // L0
            ((virt_addr >> 30) & 0x1FF) as usize, // L1
            ((virt_addr >> 21) & 0x1FF) as usize, // L2
            ((virt_addr >> 12) & 0x1FF) as usize, // L3
        ];

        // Walk L0 -> L1 -> L2 -> L3
        let mut current_table: &mut PageTable = &mut *self.level_0_table;

        for &idx in &indices[0..3] {
            let entry = current_table.entries[idx];

            let next_table_phys = if (entry & DESC_VALID) == 0 {
                let frame = frame_allocator
                    .allocate_frame()
                    .expect("Out of usable physical frames for page tables!");
                let phys = frame.start_address.0;

                // Zero out the newly allocated table via HHDM
                let next_virt = (self.physical_memory_offset.0 + phys) as *mut PageTable;
                (*next_virt).zero();

                current_table.entries[idx] = phys | DESC_VALID | DESC_TABLE;
                phys
            } else {
                assert_ne!(entry & DESC_TABLE, 0, "Cannot split huge block mapping when mapping MMIO page!");
                entry & PHYS_ADDR_MASK
            };

            let next_virt = (self.physical_memory_offset.0 + next_table_phys) as *mut PageTable;
            current_table = &mut *next_virt;
        }

        // Write final L3 4KB page descriptor with Device-nGnRnE attributes
        let mair_idx = Self::find_device_mair_index();
        current_table.entries[indices[3]] = phys_aligned
            | DESC_VALID
            | DESC_PAGE
            | DESC_AF
            | DESC_PXN
            | DESC_UXN
            | (mair_idx << 2);

        // Flush TLB for this virtual address
        asm!(
        "dsb ishst",
        "tlbi vaae1is, {va}",
        "dsb ish",
        "isb",
        va = in(reg) (virt_addr >> 12),
        );

        (virt_addr + offset_in_page) as *mut u8
    }

    fn find_device_mair_index() -> u64 {
        let mair: u64;
        unsafe {
            asm!("mrs {}, mair_el1", out(reg) mair);
        }
        for i in 0..8 {
            if ((mair >> (i * 8)) & 0xFF) == 0x00 {
                return i as u64;
            }
        }
        panic!("No Device-nGnRnE (0x00) slot found in MAIR_EL1!");
    }
}

unsafe fn active_level_0_table(physical_memory_offset: VirtAddr) -> &'static mut PageTable {
    let ttbr1: u64;
    asm!("mrs {}, ttbr1_el1", out(reg) ttbr1);

    let phys = ttbr1 & PHYS_ADDR_MASK;
    let virt = physical_memory_offset.0 + phys;
    let page_table_ptr = virt as *mut PageTable;

    &mut *page_table_ptr
}