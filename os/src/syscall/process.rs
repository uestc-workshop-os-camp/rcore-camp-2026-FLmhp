//! Process management syscalls
use alloc::sync::Arc;
use core::{mem::size_of, slice};

use crate::{
    config::PAGE_SIZE,
    loader::get_app_data_by_name,
    mm::{
        translated_byte_buffer, translated_refmut, translated_str, MapPermission, PTEFlags,
        PageTable, VirtAddr,
    },
    task::{
        add_task, current_task, current_user_token, exit_current_and_run_next,
        suspend_current_and_run_next,
    },
    timer::get_time_us,
};

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

/// task exits and submit an exit code
pub fn sys_exit(exit_code: i32) -> ! {
    trace!("kernel:pid[{}] sys_exit", current_task().unwrap().pid.0);
    exit_current_and_run_next(exit_code);
    panic!("Unreachable in sys_exit!");
}

/// current task gives up resources for other tasks
pub fn sys_yield() -> isize {
    trace!("kernel:pid[{}] sys_yield", current_task().unwrap().pid.0);
    suspend_current_and_run_next();
    0
}

pub fn sys_getpid() -> isize {
    trace!("kernel: sys_getpid pid:{}", current_task().unwrap().pid.0);
    current_task().unwrap().pid.0 as isize
}

pub fn sys_fork() -> isize {
    trace!("kernel:pid[{}] sys_fork", current_task().unwrap().pid.0);
    let current_task = current_task().unwrap();
    let new_task = current_task.fork();
    let new_pid = new_task.pid.0;
    // modify trap context of new_task, because it returns immediately after switching
    let trap_cx = new_task.inner_exclusive_access().get_trap_cx();
    // we do not have to move to next instruction since we have done it before
    // for child process, fork returns 0
    trap_cx.x[10] = 0;
    // add new task to scheduler
    add_task(new_task);
    new_pid as isize
}

pub fn sys_exec(path: *const u8) -> isize {
    trace!("kernel:pid[{}] sys_exec", current_task().unwrap().pid.0);
    let token = current_user_token();
    let path = translated_str(token, path);
    if let Some(data) = get_app_data_by_name(path.as_str()) {
        let task = current_task().unwrap();
        task.exec(data);
        0
    } else {
        -1
    }
}

/// If there is not a child process whose pid is same as given, return -1.
/// Else if there is a child process but it is still running, return -2.
pub fn sys_waitpid(pid: isize, exit_code_ptr: *mut i32) -> isize {
    trace!(
        "kernel::pid[{}] sys_waitpid [{}]",
        current_task().unwrap().pid.0,
        pid
    );
    let task = current_task().unwrap();
    // find a child process

    // ---- access current PCB exclusively
    let mut inner = task.inner_exclusive_access();
    if !inner
        .children
        .iter()
        .any(|p| pid == -1 || pid as usize == p.getpid())
    {
        return -1;
        // ---- release current PCB
    }
    let pair = inner.children.iter().enumerate().find(|(_, p)| {
        // ++++ temporarily access child PCB exclusively
        p.inner_exclusive_access().is_zombie() && (pid == -1 || pid as usize == p.getpid())
        // ++++ release child PCB
    });
    if let Some((idx, _)) = pair {
        let child = inner.children.remove(idx);
        // confirm that child will be deallocated after being removed from children list
        assert_eq!(Arc::strong_count(&child), 1);
        let found_pid = child.getpid();
        // ++++ temporarily access child PCB exclusively
        let exit_code = child.inner_exclusive_access().exit_code;
        // ++++ release child PCB
        *translated_refmut(inner.memory_set.token(), exit_code_ptr) = exit_code;
        found_pid as isize
    } else {
        -2
    }
    // ---- release current PCB automatically
}

/// YOUR JOB: get time with second and microsecond
/// HINT: You might reimplement it with virtual memory management.
/// HINT: What if [`TimeVal`] is splitted by two pages ?
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> isize {
    trace!("kernel:pid[{}] sys_get_time", current_task().unwrap().pid.0);
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

/// YOUR JOB: Implement mmap.
pub fn sys_mmap(start: usize, len: usize, port: usize) -> isize {
    trace!("kernel:pid[{}] sys_mmap", current_task().unwrap().pid.0);
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
    current_task()
        .unwrap()
        .inner_exclusive_access()
        .memory_set
        .insert_framed_area(start.into(), end.into(), permission);
    0
}

/// YOUR JOB: Implement munmap.
pub fn sys_munmap(start: usize, len: usize) -> isize {
    trace!("kernel:pid[{}] sys_munmap", current_task().unwrap().pid.0);
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
    current_task()
        .unwrap()
        .inner_exclusive_access()
        .memory_set
        .remove_framed_area(start.into(), end.into());
    0
}

/// change data segment size
pub fn sys_sbrk(size: i32) -> isize {
    trace!("kernel:pid[{}] sys_sbrk", current_task().unwrap().pid.0);
    if let Some(old_brk) = current_task().unwrap().change_program_brk(size) {
        old_brk as isize
    } else {
        -1
    }
}

/// YOUR JOB: Implement spawn.
/// HINT: fork + exec =/= spawn
pub fn sys_spawn(path: *const u8) -> isize {
    trace!("kernel:pid[{}] sys_spawn", current_task().unwrap().pid.0);
    let path = translated_str(current_user_token(), path);
    let Some(data) = get_app_data_by_name(path.as_str()) else {
        return -1;
    };
    let task = current_task().unwrap().spawn(data);
    let pid = task.getpid();
    add_task(task);
    pid as isize
}

// YOUR JOB: Set task priority.
pub fn sys_set_priority(prio: isize) -> isize {
    trace!(
        "kernel:pid[{}] sys_set_priority",
        current_task().unwrap().pid.0
    );
    if prio <= 1 {
        return -1;
    }
    current_task().unwrap().inner_exclusive_access().priority = prio as usize;
    prio
}
