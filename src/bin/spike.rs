//! Spike 1 from the plan. It answers three risks before any UI code exists.
//!
//! Risk A: does a simple NPU counter exist, and does it match Task Manager?
//! Risk B: how heavy is the `GPU Engine(*)` array read?
//! Risk C: which source gives the CPU base clock?
//!
//! Run it with `cargo run --bin spike`. Pass `--watch` for a longer live run.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use glint::adapters::{self, AdapterKind};
use glint::luid::{engine_family, parse_engine_instance, AdapterLuid};
use glint::pdh::Query;
use glint::wddm::{self, WddmAdapter};

const GPU_ENGINE: &str = r"\GPU Engine(*)\Utilization Percentage";
const CPU_UTILITY: &str = r"\Processor Information(_Total)\% Processor Utility";
const CPU_PERFORMANCE: &str = r"\Processor Information(_Total)\% Processor Performance";
const DISK_IDLE: &str = r"\PhysicalDisk(_Total)\% Idle Time";

fn main() {
    let watch = std::env::args().any(|a| a == "--watch");

    println!("=== Risk C: CPU base clock source ===");
    risk_c();

    println!();
    println!("=== Risk A: DXCore adapters ===");
    let adapters = risk_a_adapters();

    println!();
    println!("=== Risk A: WDDM adapters ===");
    let wddm = risk_a_wddm();

    println!();
    println!("=== Risk B: GPU Engine array cost ===");
    risk_b();

    println!();
    println!("=== Risk A: live per-adapter utilization ===");
    risk_a_live(&adapters, &wddm, watch);
}

// ---------------------------------------------------------------- Risk C ----

fn risk_c() {
    match base_clock_from_registry() {
        Some(mhz) => println!("registry ~MHz          : {mhz} MHz"),
        None => println!("registry ~MHz          : unavailable"),
    }
    match max_mhz_from_power() {
        Some((max, current, limit)) => println!(
            "CallNtPowerInformation : MaxMhz {max}, CurrentMhz {current}, MhzLimit {limit}"
        ),
        None => println!("CallNtPowerInformation : unavailable"),
    }
    println!("Compare both with the Task Manager Speed readout at idle.");
}

fn base_clock_from_registry() -> Option<u32> {
    use windows::core::{w, PCWSTR};
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD};

    let mut value = 0u32;
    let mut size = size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0"),
            PCWSTR(w!("~MHz").as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut value).cast::<core::ffi::c_void>()),
            Some(&mut size),
        )
    };
    if status.is_ok() {
        Some(value)
    } else {
        None
    }
}

fn max_mhz_from_power() -> Option<(u32, u32, u32)> {
    use windows::Win32::System::Power::{
        CallNtPowerInformation, ProcessorInformation, PROCESSOR_POWER_INFORMATION,
    };
    use windows::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};

    let mut info = SYSTEM_INFO::default();
    unsafe { GetSystemInfo(&mut info) };
    let count = info.dwNumberOfProcessors.max(1) as usize;

    let mut buffer = vec![PROCESSOR_POWER_INFORMATION::default(); count];
    let bytes = (count * size_of::<PROCESSOR_POWER_INFORMATION>()) as u32;
    let status = unsafe {
        CallNtPowerInformation(
            ProcessorInformation,
            None,
            0,
            Some(buffer.as_mut_ptr().cast::<core::ffi::c_void>()),
            bytes,
        )
    };
    if status.0 != 0 {
        return None;
    }
    let first = buffer.first()?;
    Some((first.MaxMhz, first.CurrentMhz, first.MhzLimit))
}

// ---------------------------------------------------------------- Risk A ----

fn risk_a_adapters() -> Vec<adapters::Adapter> {
    let list = match adapters::enumerate() {
        Ok(list) => list,
        Err(error) => {
            println!("DXCore enumeration failed: {error}");
            return Vec::new();
        }
    };
    for adapter in &list {
        println!(
            "luid 0x{:08X}_0x{:08X}  {:?}  hw={}  integrated={}  {}",
            adapter.luid.high,
            adapter.luid.low,
            adapter.kind,
            adapter.is_hardware,
            adapter.is_integrated,
            adapter.description
        );
    }
    match adapters::primary_gpu(&list) {
        Some(gpu) => println!(
            "-> GPU row uses 0x{:08X}_0x{:08X}",
            gpu.luid.high, gpu.luid.low
        ),
        None => println!("-> no graphics adapter found"),
    }
    match adapters::npu(&list) {
        Some(npu) => println!(
            "-> NPU row uses 0x{:08X}_0x{:08X}",
            npu.luid.high, npu.luid.low
        ),
        None => println!("-> no NPU found; GLINT-NPU-OPTIONAL applies"),
    }
    list
}

fn risk_a_wddm() -> Vec<WddmAdapter> {
    let list = wddm::enumerate();
    if list.is_empty() {
        println!("D3DKMTEnumAdapters2 returned nothing");
        return list;
    }
    for adapter in &list {
        println!(
            "luid 0x{:08X}_0x{:08X}  compute_only={:<5} render={:<5} display={:<5} sw={:<5} {}",
            adapter.luid.high,
            adapter.luid.low,
            adapter.compute_only,
            adapter.render_supported,
            adapter.display_supported,
            adapter.software_device,
            adapter.name
        );
    }
    match wddm::primary_gpu(&list) {
        Some(gpu) => println!(
            "-> GPU row uses 0x{:08X}_0x{:08X}",
            gpu.luid.high, gpu.luid.low
        ),
        None => println!("-> no render adapter found"),
    }
    match wddm::npu(&list) {
        Some(npu) => println!(
            "-> NPU row uses 0x{:08X}_0x{:08X}  ({})",
            npu.luid.high, npu.luid.low, npu.name
        ),
        None => println!("-> no NPU found; GLINT-NPU-OPTIONAL applies"),
    }
    list
}

// ---------------------------------------------------------------- Risk B ----

fn risk_b() {
    let query = match Query::new() {
        Ok(query) => query,
        Err(error) => return println!("PdhOpenQuery failed: {error}"),
    };
    let mut gpu = match query.add(GPU_ENGINE) {
        Ok(counter) => counter,
        Err(error) => return println!("add GPU Engine failed: {error}"),
    };

    // Two collects: the first one primes the rate counters.
    let _ = query.collect();
    std::thread::sleep(Duration::from_millis(1000));
    let _ = query.collect();

    let mut instances = 0usize;
    let _ = gpu.for_each_instance(|_, _| instances += 1);
    println!("instances in the array : {instances}");

    const ROUNDS: u32 = 100;
    let mut collect_total = Duration::ZERO;
    let mut array_total = Duration::ZERO;
    let mut worst = Duration::ZERO;
    for _ in 0..ROUNDS {
        let start = Instant::now();
        let _ = query.collect();
        let after_collect = Instant::now();
        let mut seen = 0usize;
        let _ = gpu.for_each_instance(|_, _| seen += 1);
        let end = Instant::now();

        collect_total += after_collect - start;
        array_total += end - after_collect;
        worst = worst.max(end - start);
    }
    let per = |total: Duration| total.as_secs_f64() * 1000.0 / f64::from(ROUNDS);
    println!("PdhCollectQueryData    : {:.3} ms mean", per(collect_total));
    println!("GetFormattedArray+parse: {:.3} ms mean", per(array_total));
    println!(
        "total per tick         : {:.3} ms mean, {:.3} ms worst",
        per(collect_total) + per(array_total),
        worst.as_secs_f64() * 1000.0
    );
    println!("Exit criterion: under 2 ms per call.");
}

// ----------------------------------------------------------- Risk A, live ----

/// Sum `GPU Engine` utilization per adapter and per engine family, then take
/// the largest family. This is the number that Task Manager shows.
fn risk_a_live(adapter_list: &[adapters::Adapter], wddm_list: &[WddmAdapter], watch: bool) {
    let query = match Query::new() {
        Ok(query) => query,
        Err(error) => return println!("PdhOpenQuery failed: {error}"),
    };
    let mut gpu = match query.add(GPU_ENGINE) {
        Ok(counter) => counter,
        Err(error) => return println!("add GPU Engine failed: {error}"),
    };
    let cpu_utility = query.add(CPU_UTILITY).ok();
    let cpu_performance = query.add(CPU_PERFORMANCE).ok();
    let disk_idle = query.add(DISK_IDLE).ok();

    let base_mhz = base_clock_from_registry().unwrap_or(0);
    let rounds = if watch { 30 } else { 3 };

    let _ = query.collect();
    for round in 0..rounds {
        std::thread::sleep(Duration::from_millis(1000));
        let _ = query.collect();

        // adapter -> engine family -> summed utilization
        let mut per_adapter: BTreeMap<AdapterLuid, BTreeMap<String, f64>> = BTreeMap::new();
        let mut bad = 0usize;
        let _ = gpu.for_each_instance(|name, value| match parse_engine_instance(name) {
            Ok(instance) => {
                *per_adapter
                    .entry(instance.luid)
                    .or_default()
                    .entry(engine_family(instance.engtype).to_string())
                    .or_insert(0.0) += value;
            }
            Err(_) => bad += 1,
        });

        let cpu = cpu_utility
            .as_ref()
            .and_then(|c| c.value().ok())
            .unwrap_or(0.0);
        let performance = cpu_performance
            .as_ref()
            .and_then(|c| c.value_nocap().ok())
            .unwrap_or(0.0);
        let disk = disk_idle
            .as_ref()
            .and_then(|c| c.value().ok())
            .unwrap_or(100.0);
        let ghz = f64::from(base_mhz) * performance / 100_000.0;

        println!(
            "--- round {} --- CPU {:.0}%  {:.2} GHz ({:.2}x)  DISK {:.0}%  unparsed {}",
            round + 1,
            cpu.clamp(0.0, 100.0),
            ghz,
            performance / 100.0,
            (100.0 - disk).clamp(0.0, 100.0),
            bad
        );
        for (luid, families) in &per_adapter {
            // Prefer the WDDM record: it sees an NPU that DXCore misses.
            let label = wddm_list
                .iter()
                .find(|a| a.luid == *luid)
                .map(|a| {
                    let kind = if a.is_npu() { "NPU" } else { "GPU" };
                    format!("{kind} {}", a.name)
                })
                .or_else(|| {
                    adapter_list.iter().find(|a| a.luid == *luid).map(|a| {
                        let kind = if a.kind == AdapterKind::ComputeOnly {
                            "NPU"
                        } else {
                            "GPU"
                        };
                        format!("{kind} {}", a.description)
                    })
                })
                .unwrap_or_else(|| "unknown adapter".to_string());
            let top = families
                .values()
                .copied()
                .fold(0.0f64, f64::max)
                .clamp(0.0, 100.0);
            let detail: Vec<String> = families
                .iter()
                .filter(|(_, sum)| **sum > 0.05)
                .map(|(family, sum)| format!("{family} {sum:.1}"))
                .collect();
            println!(
                "  0x{:08X}_0x{:08X} {:<34} {:>5.1}%   [{}]",
                luid.high,
                luid.low,
                label,
                top,
                detail.join(", ")
            );
        }
    }
    println!("Exit criterion: the NPU number tracks the Task Manager graph within ~5 points.");
}
