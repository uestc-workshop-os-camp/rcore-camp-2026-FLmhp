//! Process management syscalls
use crate::{
    config::PAGE_SIZE,
    mm::{translated_byte_buffer, MapPermission, PTEFlags, PageTable, VirtAddr},
    task::{
        change_program_brk, current_user_token, exit_current_and_run_next, map_current,
        suspend_current_and_run_next, syscall_count, unmap_current,
    },
    timer::get_time_us,
};
use core::{mem::size_of, slice};

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

/// task exits and submit an exit code
pub fn sys_exit(_exit_code: i32) -> ! {
    trace!("kernel: sys_exit");
    exit_current_and_run_next();
    panic!("Unreachable in sys_exit!");
}

/// current task gives up resources for other tasks
pub fn sys_yield() -> isize {
    trace!("kernel: sys_yield");
    suspend_current_and_run_next();
    0
}

/// YOUR JOB: get time with second and microsecond
/// HINT: You might reimplement it with virtual memory management.
/// HINT: What if [`TimeVal`] is splitted by two pages ?
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> isize {
    trace!("kernel: sys_get_time");
    let us = get_time_us();
    let value = TimeVal {
        sec: us / 1_000_000,
        usec: us % 1_000_000,
    };
    let start = ts as usize;
    let len = size_of::<TimeVal>();
    let Some(end) = start.checked_add(len) else {
        return -1;
    };
    if end > 1 << 38 {
        return -1;
    }
    let token = current_user_token();
    let page_table = PageTable::from_token(token);
    for address in (start..end).step_by(PAGE_SIZE) {
        let Some(pte) = page_table.translate(VirtAddr::from(address).floor()) else {
            return -1;
        };
        if !pte.is_valid() || !pte.writable() || !pte.flags().contains(PTEFlags::U) {
            return -1;
        }
    }
    if start / PAGE_SIZE != (end - 1) / PAGE_SIZE {
        let Some(pte) = page_table.translate(VirtAddr::from(end - 1).floor()) else {
            return -1;
        };
        if !pte.is_valid() || !pte.writable() || !pte.flags().contains(PTEFlags::U) {
            return -1;
        }
    }
    let bytes = unsafe { slice::from_raw_parts((&value as *const TimeVal).cast::<u8>(), len) };
    let mut copied = 0;
    for buffer in translated_byte_buffer(token, ts.cast::<u8>(), len) {
        buffer.copy_from_slice(&bytes[copied..copied + buffer.len()]);
        copied += buffer.len();
    }
    0
}

/// TODO: Finish sys_trace to pass testcases
/// HINT: You might reimplement it with virtual memory management.
pub fn sys_trace(trace_request: usize, id: usize, data: usize) -> isize {
    trace!("kernel: sys_trace");
    if trace_request == 2 {
        return syscall_count(id).map_or(-1, |count| count as isize);
    }
    if id >= 1 << 38 {
        return -1;
    }
    let page_table = PageTable::from_token(current_user_token());
    let address = VirtAddr::from(id);
    let Some(pte) = page_table.translate(address.floor()) else {
        return -1;
    };
    if !pte.is_valid() || !pte.flags().contains(PTEFlags::U) {
        return -1;
    }
    let byte = &mut pte.ppn().get_bytes_array()[address.page_offset()];
    match trace_request {
        0 if pte.readable() => *byte as isize,
        1 if pte.writable() => {
            *byte = data as u8;
            0
        }
        _ => -1,
    }
}

// YOUR JOB: Implement mmap.
pub fn sys_mmap(start: usize, len: usize, port: usize) -> isize {
    trace!("kernel: sys_mmap");
    let Some(end) = start.checked_add(len) else {
        return -1;
    };
    if start % PAGE_SIZE != 0 || port == 0 || port & !0x7 != 0 || end > 1 << 38 {
        return -1;
    }
    if len == 0 {
        return 0;
    }
    let page_table = PageTable::from_token(current_user_token());
    for page in 0..len.div_ceil(PAGE_SIZE) {
        let vpn = VirtAddr::from(start + page * PAGE_SIZE).floor();
        if page_table.translate(vpn).is_some_and(|pte| pte.is_valid()) {
            return -1;
        }
    }
    let mut permission = MapPermission::U;
    if port & 1 != 0 {
        permission |= MapPermission::R;
    }
    if port & 2 != 0 {
        permission |= MapPermission::W;
    }
    if port & 4 != 0 {
        permission |= MapPermission::X;
    }
    map_current(start.into(), end.into(), permission);
    0
}

// YOUR JOB: Implement munmap.
pub fn sys_munmap(start: usize, len: usize) -> isize {
    trace!("kernel: sys_munmap");
    let Some(end) = start.checked_add(len) else {
        return -1;
    };
    if start % PAGE_SIZE != 0 || end > 1 << 38 {
        return -1;
    }
    if len == 0 {
        return 0;
    }
    let page_table = PageTable::from_token(current_user_token());
    for page in 0..len.div_ceil(PAGE_SIZE) {
        let vpn = VirtAddr::from(start + page * PAGE_SIZE).floor();
        let Some(pte) = page_table.translate(vpn) else {
            return -1;
        };
        if !pte.is_valid() || !pte.flags().contains(PTEFlags::U) {
            return -1;
        }
    }
    unmap_current(start.into(), end.into());
    0
}
/// change data segment size
pub fn sys_sbrk(size: i32) -> isize {
    trace!("kernel: sys_sbrk");
    if let Some(old_brk) = change_program_brk(size) {
        old_brk as isize
    } else {
        -1
    }
}
