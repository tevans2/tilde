//! Process counters before and after an operation: time, CPU, instructions, cycles,
//! energy, memory, page faults, disk I/O, wakeups and context switches, from
//! `proc_pid_rusage` and `getrusage`, plus the allocator's counts. All are process-wide,
//! so work on background threads (the index's matcher, FSEvents) counts too.

use std::sync::OnceLock;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::alloc;

/// A snapshot of the counters.
#[derive(Debug, Clone, Copy)]
pub struct Usage {
    at: Instant,
    user_ns: u64,
    system_ns: u64,
    instructions: u64,
    cycles: u64,
    energy_nj: u64,
    footprint: u64,
    pageins: u64,
    disk_read: u64,
    disk_written: u64,
    idle_wakeups: u64,
    runnable_ns: u64,
    minor_faults: i64,
    voluntary_switches: i64,
    involuntary_switches: i64,
    heap: alloc::Heap,
}

/// What an operation cost: the difference between two snapshots.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct Cost {
    /// Wall-clock time.
    pub wall_ns: f64,
    /// CPU time on every thread, user + system.
    pub cpu_ns: f64,
    pub instructions: f64,
    pub cycles: f64,
    /// Energy the kernel billed to the process (nanojoules).
    pub energy_nj: f64,
    pub allocs: f64,
    pub alloc_bytes: f64,
    /// Change in heap bytes held.
    pub heap_delta: f64,
    /// Change in physical footprint (what Activity Monitor shows as Memory).
    pub footprint_delta: f64,
    pub minor_faults: f64,
    pub pageins: f64,
    pub disk_read: f64,
    pub disk_written: f64,
    /// Package idle wakeups: what keeps a laptop from sleeping its CPU.
    pub idle_wakeups: f64,
    /// Time threads were ready but waiting for a CPU.
    pub runnable_ns: f64,
    pub context_switches: f64,
}

pub const METRICS: &[&str] = &[
    "wall_ns",
    "cpu_ns",
    "instructions",
    "cycles",
    "energy_nj",
    "allocs",
    "alloc_bytes",
    "heap_delta",
    "footprint_delta",
    "minor_faults",
    "pageins",
    "disk_read",
    "disk_written",
    "idle_wakeups",
    "runnable_ns",
    "context_switches",
];

impl Cost {
    pub fn get(&self, metric: &str) -> f64 {
        match metric {
            "wall_ns" => self.wall_ns,
            "cpu_ns" => self.cpu_ns,
            "instructions" => self.instructions,
            "cycles" => self.cycles,
            "energy_nj" => self.energy_nj,
            "allocs" => self.allocs,
            "alloc_bytes" => self.alloc_bytes,
            "heap_delta" => self.heap_delta,
            "footprint_delta" => self.footprint_delta,
            "minor_faults" => self.minor_faults,
            "pageins" => self.pageins,
            "disk_read" => self.disk_read,
            "disk_written" => self.disk_written,
            "idle_wakeups" => self.idle_wakeups,
            "runnable_ns" => self.runnable_ns,
            "context_switches" => self.context_switches,
            _ => f64::NAN,
        }
    }

    /// Per operation, when a sample ran the operation `n` times.
    pub fn per(self, n: u64) -> Cost {
        let n = n.max(1) as f64;
        Cost {
            wall_ns: self.wall_ns / n,
            cpu_ns: self.cpu_ns / n,
            instructions: self.instructions / n,
            cycles: self.cycles / n,
            energy_nj: self.energy_nj / n,
            allocs: self.allocs / n,
            alloc_bytes: self.alloc_bytes / n,
            heap_delta: self.heap_delta / n,
            footprint_delta: self.footprint_delta / n,
            minor_faults: self.minor_faults / n,
            pageins: self.pageins / n,
            disk_read: self.disk_read / n,
            disk_written: self.disk_written / n,
            idle_wakeups: self.idle_wakeups / n,
            runnable_ns: self.runnable_ns / n,
            context_switches: self.context_switches / n,
        }
    }
}

/// Mach absolute time → nanoseconds (1:1 on Intel, 125/3 on Apple silicon).
fn timebase() -> (u64, u64) {
    static TB: OnceLock<(u64, u64)> = OnceLock::new();
    *TB.get_or_init(|| {
        #[repr(C)]
        struct Timebase {
            numer: u32,
            denom: u32,
        }
        unsafe extern "C" {
            fn mach_timebase_info(info: *mut Timebase) -> i32;
        }
        let mut info = Timebase { numer: 0, denom: 0 };
        // SAFETY: fills in a plain struct.
        unsafe { mach_timebase_info(&mut info) };
        (info.numer.max(1) as u64, info.denom.max(1) as u64)
    })
}

fn abs_to_ns(t: u64) -> u64 {
    let (numer, denom) = timebase();
    ((t as u128 * numer as u128) / denom as u128) as u64
}

pub fn usage() -> Usage {
    // SAFETY: plain structs filled in by the kernel.
    let (ri, ru) = unsafe {
        let mut ri: libc::rusage_info_v4 = std::mem::zeroed();
        libc::proc_pid_rusage(
            libc::getpid(),
            libc::RUSAGE_INFO_V4,
            (&mut ri as *mut libc::rusage_info_v4).cast::<libc::rusage_info_t>(),
        );
        let mut ru: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut ru);
        (ri, ru)
    };
    Usage {
        at: Instant::now(),
        user_ns: abs_to_ns(ri.ri_user_time),
        system_ns: abs_to_ns(ri.ri_system_time),
        instructions: ri.ri_instructions,
        cycles: ri.ri_cycles,
        energy_nj: ri.ri_billed_energy,
        footprint: ri.ri_phys_footprint,
        pageins: ri.ri_pageins,
        disk_read: ri.ri_diskio_bytesread,
        disk_written: ri.ri_diskio_byteswritten,
        idle_wakeups: ri.ri_pkg_idle_wkups,
        runnable_ns: abs_to_ns(ri.ri_runnable_time),
        minor_faults: ru.ru_minflt,
        voluntary_switches: ru.ru_nvcsw,
        involuntary_switches: ru.ru_nivcsw,
        heap: alloc::heap(),
    }
}

/// Bytes of physical memory the process holds now.
pub fn footprint() -> u64 {
    usage().footprint
}

impl Usage {
    pub fn since(&self, before: &Usage) -> Cost {
        let d = |a: u64, b: u64| a.saturating_sub(b) as f64;
        Cost {
            wall_ns: self.at.duration_since(before.at).as_nanos() as f64,
            cpu_ns: d(self.user_ns + self.system_ns, before.user_ns + before.system_ns),
            instructions: d(self.instructions, before.instructions),
            cycles: d(self.cycles, before.cycles),
            energy_nj: d(self.energy_nj, before.energy_nj),
            allocs: d(self.heap.allocs, before.heap.allocs),
            alloc_bytes: d(self.heap.bytes, before.heap.bytes),
            heap_delta: (self.heap.live - before.heap.live) as f64,
            footprint_delta: self.footprint as f64 - before.footprint as f64,
            minor_faults: (self.minor_faults - before.minor_faults) as f64,
            pageins: d(self.pageins, before.pageins),
            disk_read: d(self.disk_read, before.disk_read),
            disk_written: d(self.disk_written, before.disk_written),
            idle_wakeups: d(self.idle_wakeups, before.idle_wakeups),
            runnable_ns: d(self.runnable_ns, before.runnable_ns),
            context_switches: ((self.voluntary_switches + self.involuntary_switches)
                - (before.voluntary_switches + before.involuntary_switches)) as f64,
        }
    }
}
