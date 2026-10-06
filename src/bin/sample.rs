//! Milestone 2: the sampler with console output, before any UI exists.
//!
//! Run `cargo run --release --bin sample`. Compare each number with Task
//! Manager. Pass a tick count to change the run length, such as `-- 30`.
//!
//! It opens with a wiring report. A 0% row has two very different causes:
//! the resource is idle, or the row matches no counter instance at all. The
//! report separates them, so an NPU row that reads 0% forever cannot hide a
//! wrong LUID.

use std::collections::BTreeMap;
use std::time::Duration;

use glint::luid::{parse_engine_instance, AdapterLuid};
use glint::pdh::Query;
use glint::sampler::Sampler;
use glint::wddm;

fn gib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0 * 1024.0)
}

/// Count the `GPU Engine` instances that each adapter LUID owns, and name the
/// adapters the way the app picks them.
fn report_wiring() {
    let adapters = wddm::enumerate();
    let gpu = wddm::primary_gpu(&adapters).map(|a| a.luid);
    let npu = wddm::npu(&adapters).map(|a| a.luid);

    let mut counts: BTreeMap<AdapterLuid, usize> = BTreeMap::new();
    let mut unparsed = 0usize;
    if let Ok(query) = Query::new()
        && let Ok(mut engines) = query.add(r"\GPU Engine(*)\Utilization Percentage")
    {
        let _ = query.collect();
        std::thread::sleep(Duration::from_millis(1000));
        let _ = query.collect();
        let _ = engines.for_each_instance(|name, _| match parse_engine_instance(name) {
            Ok(instance) => *counts.entry(instance.luid).or_insert(0) += 1,
            Err(_) => unparsed += 1,
        });
    }

    println!("--- wiring ---");
    for adapter in &adapters {
        let role = if Some(adapter.luid) == gpu {
            "GPU row"
        } else if Some(adapter.luid) == npu {
            "NPU row"
        } else {
            "unused"
        };
        let instances = counts.get(&adapter.luid).copied().unwrap_or(0);
        let name = if adapter.name.is_empty() {
            "(no driver name)"
        } else {
            adapter.name.as_str()
        };
        println!(
            "  0x{:08X}_0x{:08X}  {:<8} {:>4} instances  {}",
            adapter.luid.high, adapter.luid.low, role, instances, name
        );
        if role != "unused" && instances == 0 {
            println!("    WARNING: this row matches no counter, so it will read 0% forever");
        }
    }
    if unparsed > 0 {
        println!("  {unparsed} instance names did not parse");
    }
    if npu.is_none() {
        println!("  no NPU on this machine; the NPU row is absent (GLINT-NPU-OPTIONAL)");
    }
    println!();
}

fn main() {
    let rounds: u32 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(10);

    report_wiring();

    let mut sampler = match Sampler::new() {
        Ok(sampler) => sampler,
        Err(error) => return println!("sampler failed to start: {error}"),
    };

    for round in 0..=rounds {
        std::thread::sleep(Duration::from_millis(1000));
        let metrics = sampler.tick();
        if round == 0 {
            println!("(first tick primes the rate counters)");
            continue;
        }

        let gpu = metrics
            .gpu
            .map(|value| format!("{value:4.0}%"))
            .unwrap_or_else(|| "  --".to_string());
        let npu = metrics
            .npu
            .map(|value| format!("{value:4.0}%"))
            .unwrap_or_else(|| "  --".to_string());

        println!(
            "CPU {:3.0}% {:.2} GHz ({:.2}x) | MEM {:3.0}% {:.1}/{:.1} GiB | GPU {} | NPU {} | DISK {:3.0}%",
            metrics.cpu,
            metrics.cpu_ghz,
            metrics.cpu_ratio,
            metrics.memory,
            gib(metrics.memory_used_bytes),
            gib(metrics.memory_total_bytes),
            gpu,
            npu,
            metrics.disk,
        );

        if round == 1 {
            for volume in &metrics.volumes {
                println!(
                    "  {} {:.0}% used, {:.1}/{:.1} GiB",
                    volume.label,
                    volume.percent(),
                    gib(volume.used_bytes),
                    gib(volume.total_bytes)
                );
            }
        }
    }
}
