//! What `/proc` says about the machine and the processes under test.

use std::path::{Path, PathBuf};

/// The machine a run measured.
pub struct Machine {
    pub cpu: String,
    pub ram_bytes: u64,
}

impl Machine {
    pub fn read() -> Self {
        let cpu = std::fs::read_to_string("/proc/cpuinfo")
            .ok()
            .and_then(|text| cpu_model(&text))
            .unwrap_or_else(|| "unknown".to_string());
        let ram_bytes = std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|text| kib_field(&text, "MemTotal:"))
            .unwrap_or(0);
        Self { cpu, ram_bytes }
    }
}

/// The one-minute load average.
pub fn load_average() -> Option<f64> {
    std::fs::read_to_string("/proc/loadavg")
        .ok()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// Resident plus swapped memory of `pid`, in bytes.
pub fn memory_of(pid: u32) -> Option<u64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    resident_and_swapped(&text)
}

/// Every process descended from `pid`.
pub fn descendants_of(pid: u32) -> Vec<u32> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let parents: Vec<(u32, u32)> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        .filter_map(|child| {
            let stat = std::fs::read_to_string(format!("/proc/{child}/stat")).ok()?;
            Some((child, parent_in_stat(&stat)?))
        })
        .collect();
    let mut found = vec![pid];
    let mut index = 0;
    while index < found.len() {
        let parent = found[index];
        found.extend(
            parents
                .iter()
                .filter(|(_, of)| *of == parent)
                .map(|(child, _)| *child),
        );
        index += 1;
    }
    found.remove(0);
    found
}

/// The descendants of `pid` running an executable from under `dir`.
pub fn descendants_running_from(pid: u32, dir: &Path) -> Vec<(u32, PathBuf)> {
    descendants_of(pid)
        .into_iter()
        .filter_map(|child| {
            let exe = std::fs::read_link(format!("/proc/{child}/exe")).ok()?;
            exe.starts_with(dir).then_some((child, exe))
        })
        .collect()
}

fn cpu_model(cpuinfo: &str) -> Option<String> {
    cpuinfo
        .lines()
        .find_map(|line| line.strip_prefix("model name"))
        .and_then(|rest| rest.split_once(':'))
        .map(|(_, model)| model.trim().to_string())
}

/// A `Name:   1234 kB` field, in bytes.
fn kib_field(text: &str, name: &str) -> Option<u64> {
    let line = text.lines().find(|line| line.starts_with(name))?;
    let kib: u64 = line[name.len()..].split_whitespace().next()?.parse().ok()?;
    Some(kib * 1024)
}

fn resident_and_swapped(status: &str) -> Option<u64> {
    let resident = kib_field(status, "VmRSS:")?;
    Some(resident + kib_field(status, "VmSwap:").unwrap_or(0))
}

/// The parent pid in a `/proc/<pid>/stat` line, whose command name may hold
/// spaces and parentheses.
fn parent_in_stat(stat: &str) -> Option<u32> {
    let (_, rest) = stat.rsplit_once(')')?;
    rest.split_whitespace().nth(1)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_counts_resident_and_swapped_pages() {
        let status = "Name:\tjackdaw\nVmRSS:\t  2048 kB\nRssAnon:\t 1000 kB\nVmSwap:\t   512 kB\n";
        assert_eq!(resident_and_swapped(status), Some(2560 * 1024));
    }

    #[test]
    fn a_process_with_no_swap_line_counts_resident_only() {
        assert_eq!(resident_and_swapped("VmRSS:\t 10 kB\n"), Some(10 * 1024));
        assert_eq!(resident_and_swapped("Name:\tzombie\n"), None);
    }

    #[test]
    fn the_parent_pid_survives_a_command_name_with_parentheses() {
        assert_eq!(
            parent_in_stat("4321 (my (game) x) S 1234 4321 1 0"),
            Some(1234)
        );
        assert_eq!(parent_in_stat("garbage"), None);
    }

    #[test]
    fn the_cpu_model_is_the_first_model_name() {
        let text = "processor\t: 0\nmodel name\t: Some CPU @ 2.60GHz\nprocessor\t: 1\nmodel name\t: other\n";
        assert_eq!(cpu_model(text).as_deref(), Some("Some CPU @ 2.60GHz"));
    }
}
