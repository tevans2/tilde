//! What a run was measured on, and the conditions that make numbers unreliable (battery,
//! Low Power Mode, a hot machine, other work, a debug build).

use std::ffi::CString;
use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Environment {
    pub machine: String,
    pub cpu: String,
    pub performance_cores: u32,
    pub efficiency_cores: u32,
    pub memory_gb: f64,
    pub macos: String,
    pub rustc: String,
    pub release_build: bool,
    pub commit: String,
    /// Uncommitted changes in the repository.
    pub dirty: bool,
    pub power: String,
    pub low_power_mode: bool,
    /// From `pmset -g therm`: 100 unless the CPU is being slowed for heat.
    pub cpu_speed_limit: u32,
    pub load_before: [f64; 3],
    pub load_after: [f64; 3],
}

impl Environment {
    /// Conditions likely to skew the numbers.
    pub fn warnings(&self) -> Vec<String> {
        let mut w = Vec::new();
        if !self.release_build {
            w.push("debug build: run with --release".into());
        }
        if self.power != "AC" {
            w.push(format!("on {} power: plug in for steady numbers", self.power));
        }
        if self.low_power_mode {
            w.push("Low Power Mode is on".into());
        }
        if self.cpu_speed_limit < 100 {
            w.push(format!("the CPU is slowed for heat (speed limit {}%)", self.cpu_speed_limit));
        }
        let cores = (self.performance_cores + self.efficiency_cores).max(1) as f64;
        if self.load_before[0] > cores * 0.25 {
            w.push(format!("the machine was busy (load {:.1}): close other apps", self.load_before[0]));
        }
        w
    }
}

fn sysctl_string(name: &str) -> String {
    let Ok(c) = CString::new(name) else { return String::new() };
    let mut len = 0usize;
    // SAFETY: first asks for the length, then fills a buffer of that length.
    unsafe {
        if libc::sysctlbyname(c.as_ptr(), std::ptr::null_mut(), &mut len, std::ptr::null_mut(), 0) != 0 {
            return String::new();
        }
        let mut buf = vec![0u8; len];
        if libc::sysctlbyname(c.as_ptr(), buf.as_mut_ptr().cast(), &mut len, std::ptr::null_mut(), 0) != 0 {
            return String::new();
        }
        String::from_utf8_lossy(&buf[..len]).trim_end_matches('\0').to_string()
    }
}

fn sysctl_u64(name: &str) -> u64 {
    let Ok(c) = CString::new(name) else { return 0 };
    let mut value = 0u64;
    let mut len = std::mem::size_of::<u64>();
    // SAFETY: an integer of the size given.
    let ok = unsafe {
        libc::sysctlbyname(c.as_ptr(), (&mut value as *mut u64).cast(), &mut len, std::ptr::null_mut(), 0) == 0
    };
    if !ok {
        return 0;
    }
    // 32-bit values fill the low half.
    if len == 4 { value & 0xFFFF_FFFF } else { value }
}

fn run(program: &str, args: &[&str], dir: Option<&Path>) -> String {
    let mut cmd = Command::new(program);
    cmd.args(args);
    if let Some(dir) = dir {
        cmd.current_dir(dir);
    }
    cmd.output().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default()
}

pub fn load_average() -> [f64; 3] {
    let mut l = [0f64; 3];
    // SAFETY: room for three values.
    unsafe { libc::getloadavg(l.as_mut_ptr(), 3) };
    l
}

pub fn capture(repo: &Path) -> Environment {
    let batt = run("/usr/bin/pmset", &["-g", "batt"], None);
    let power = if batt.contains("'AC Power'") {
        "AC"
    } else if batt.contains("'Battery Power'") {
        "battery"
    } else {
        "unknown"
    };
    let settings = run("/usr/bin/pmset", &["-g"], None);
    let low_power_mode =
        settings.lines().any(|l| l.split_whitespace().collect::<Vec<_>>().as_slice() == ["lowpowermode", "1"]);
    let therm = run("/usr/bin/pmset", &["-g", "therm"], None);
    let cpu_speed_limit = therm
        .lines()
        .find_map(|l| l.trim().strip_prefix("CPU_Speed_Limit"))
        .and_then(|v| v.trim_start_matches([' ', '=']).trim().parse().ok())
        .unwrap_or(100);
    Environment {
        machine: sysctl_string("hw.model"),
        cpu: sysctl_string("machdep.cpu.brand_string"),
        performance_cores: sysctl_u64("hw.perflevel0.physicalcpu") as u32,
        efficiency_cores: sysctl_u64("hw.perflevel1.physicalcpu") as u32,
        memory_gb: sysctl_u64("hw.memsize") as f64 / (1u64 << 30) as f64,
        macos: format!("{} ({})", sysctl_string("kern.osproductversion"), sysctl_string("kern.osversion")),
        rustc: env!("TILDE_BENCH_RUSTC").to_string(),
        release_build: !cfg!(debug_assertions),
        commit: run("/usr/bin/git", &["rev-parse", "--short", "HEAD"], Some(repo)).trim().to_string(),
        dirty: !run("/usr/bin/git", &["status", "--porcelain", "--untracked-files=no"], Some(repo)).trim().is_empty(),
        power: power.into(),
        low_power_mode,
        cpu_speed_limit,
        load_before: load_average(),
        load_after: [0.0; 3],
    }
}
