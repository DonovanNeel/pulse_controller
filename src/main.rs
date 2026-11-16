#![no_std]
#![no_main]

mod paging;

use core::panic::PanicInfo;
use limine::BaseRevision;
use limine::request::{
    HhdmRequest, MemoryMapRequest, RequestsEndMarker, RequestsStartMarker,
};
use paging::{BootInfoFrameAllocator, OffsetPageTable, PhysAddr, VirtAddr};

// Limine Protocol Markers & Requests
#[used]
#[link_section = ".requests_start_marker"]
static _START_MARKER: RequestsStartMarker = RequestsStartMarker::new();

#[used]
#[link_section = ".requests"]
static BASE_REVISION: BaseRevision = BaseRevision::new();

#[used]
#[link_section = ".requests"]
static HHDM_REQUEST: HhdmRequest = HhdmRequest::new();

#[used]
#[link_section = ".requests"]
static MEMORY_MAP_REQUEST: MemoryMapRequest = MemoryMapRequest::new();

#[used]
#[link_section = ".requests_end_marker"]
static _END_MARKER: RequestsEndMarker = RequestsEndMarker::new();

// Kernel entry point
#[no_mangle]
pub extern "C" fn _start() -> ! {
    // Verify Limine bootloader revision is supported
    assert!(BASE_REVISION.is_supported());

    // Retrieve the Higher-Half Direct Map offset for MMIO access
    let hhdm_offset = HHDM_REQUEST
        .get_response()
        .expect("Missing HHDM response")
        .offset();

    let memory_map = MEMORY_MAP_REQUEST
        .get_response()
        .expect("Missing MemoryMap response");

    let phys_mem_offset = VirtAddr(hhdm_offset);

    let mut mapper = unsafe { OffsetPageTable::init(phys_mem_offset) };
    let mut frame_allocator = unsafe { BootInfoFrameAllocator::init(memory_map) };

    // QEMU virt PL011 UART physical base is 0x0900_0000
    // TODO: change this constant to 0xFE20_1000 when moving to physical rasbpi
    let uart_virt = unsafe {
        mapper.map_mmio_page(PhysAddr(0x0900_0000), &mut frame_allocator)
    };

    for &byte in b"Hello from adapted Limine OffsetPageTable on AArch64!\r\n" {
        unsafe {
            uart_virt.write_volatile(byte);
        }
    }

    hcf();
}

fn hcf() -> ! {
    loop {
        unsafe {
            core::arch::asm!("wfi");
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    hcf();
}