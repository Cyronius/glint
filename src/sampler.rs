//! The metric sampler. One PDH query feeds every row.
//!
//! Spike 1 measured the whole tick at 0.43 ms mean, so the sampler runs on
//! the UI thread with no worker and no second poll rate.

use std::time::Instant;

use crate::luid::{parse_engine_instance, AdapterLuid};
use crate::pdh::{Counter, PdhError, Query};
use crate::wddm;

/// Samples kept per row. One minute of history at a 1 s poll.
pub const HISTORY: usize = 60;

/// Ticks between disk space reads. Free space moves slowly.
const DISK_SPACE_EVERY: u64 = 30;

const CPU_TIME: &str = r"\Processor Information(_Total)\% Processor Time";
const CPU_PERFORMANCE: &str = r"\Processor Information(_Total)\% Processor Performance";
const DISK_IDLE: &str = r"\PhysicalDisk(_Total)\% Idle Time";
const GPU_ENGINE: &str = r"\GPU Engine(*)\Utilization Percentage";
const THERMAL_ZONE: &str = r"\Thermal Zone Information(*)\Temperature";

/// Ticks between adapter scans; a scan costs about 1 ms and a read far less.
const NET_LINKS_EVERY: u64 = 10;

/// A ring of recent samples, oldest first when read.
#[derive(Clone, Copy, Debug)]
pub struct History {
    samples: [f32; HISTORY],
    next: usize,
    filled: usize,
}

impl Default for History {
    fn default() -> Self {
        Self {
            samples: [0.0; HISTORY],
            next: 0,
            filled: 0,
        }
    }
}

impl History {
    pub fn push(&mut self, value: f32) {
        self.samples[self.next] = value;
        self.next = (self.next + 1) % HISTORY;
        self.filled = (self.filled + 1).min(HISTORY);
    }

    pub fn len(&self) -> usize {
        self.filled
    }

    pub fn is_empty(&self) -> bool {
        self.filled == 0
    }

    /// Walk the samples oldest first, so a sparkline reads left to right.
    pub fn iter(&self) -> impl Iterator<Item = f32> + '_ {
        let start = (self.next + HISTORY - self.filled) % HISTORY;
        (0..self.filled).map(move |step| self.samples[(start + step) % HISTORY])
    }

    pub fn last(&self) -> f32 {
        if self.filled == 0 {
            0.0
        } else {
            self.samples[(self.next + HISTORY - 1) % HISTORY]
        }
    }
}

/// One fixed drive and its space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Volume {
    /// The root path, such as `C:`.
    pub label: String,
    pub used_bytes: u64,
    pub total_bytes: u64,
}

impl Volume {
    pub fn percent(&self) -> f32 {
        if self.total_bytes == 0 {
            0.0
        } else {
            (self.used_bytes as f64 / self.total_bytes as f64 * 100.0) as f32
        }
    }
}

/// What one tick produced. Every percent is already clamped to 0 to 100
/// (GLINT-CLAMP).
#[derive(Clone, Debug, Default)]
pub struct Metrics {
    pub cpu: f32,
    /// The current clock in GHz, from the base clock times the performance
    /// ratio (GLINT-CPU-BOOST).
    pub cpu_ghz: f32,
    /// The clock as a multiple of the base clock. 1.0 means the base clock.
    pub cpu_ratio: f32,
    pub memory: f32,
    pub memory_used_bytes: u64,
    pub memory_total_bytes: u64,
    /// `None` when no graphics adapter reports counters.
    pub gpu: Option<f32>,
    /// `None` when the machine has no NPU (GLINT-NPU-OPTIONAL).
    pub npu: Option<f32>,
    /// Disk active time, which is `100 - % Idle Time`.
    pub disk: f32,
    /// `None` until a rate exists or when there is no physical adapter (GLINT-NETWORK-METRIC).
    pub network: Option<f32>,
    /// Degrees Celsius of the hottest ACPI thermal zone, clamped to 0 to 100 (GLINT-TEMP-METRIC).
    pub temp: Option<f32>,
    pub volumes: Vec<Volume>,
}

/// One minute of history for each row.
#[derive(Clone, Copy, Debug, Default)]
pub struct Histories {
    pub cpu: History,
    pub memory: History,
    pub gpu: History,
    pub npu: History,
    pub disk: History,
    pub network: History,
    pub temp: History,
}

impl Histories {
    pub fn push(&mut self, metrics: &Metrics) {
        self.cpu.push(metrics.cpu);
        self.memory.push(metrics.memory);
        self.disk.push(metrics.disk);
        if let Some(network) = metrics.network {
            self.network.push(network);
        }
        if let Some(temp) = metrics.temp {
            self.temp.push(temp);
        }
        if let Some(gpu) = metrics.gpu {
            self.gpu.push(gpu);
        }
        if let Some(npu) = metrics.npu {
            self.npu.push(npu);
        }
    }
}

/// A running sum per engine type, keyed by a hash of the type name.
///
/// A 64-bit FNV-1a hash of under twenty short names collides with vanishing
/// probability, and the key never leaves this struct. The payoff is a tick
/// that allocates nothing: a `Vec` of `Copy` pairs keeps its buffer across
/// `clear`, which a `Vec<String>` cannot.
#[derive(Default)]
struct EngineSums {
    rows: Vec<(u64, f32)>,
}

fn hash(text: &str) -> u64 {
    let mut value = 0xcbf2_9ce4_8422_2325_u64;
    for byte in text.as_bytes() {
        value ^= u64::from(*byte);
        value = value.wrapping_mul(0x0000_0100_0000_01b3);
    }
    value
}

impl EngineSums {
    fn clear(&mut self) {
        self.rows.clear();
    }

    fn add(&mut self, engine_type: &str, value: f64) {
        let key = hash(engine_type);
        match self.rows.iter_mut().find(|(seen, _)| *seen == key) {
            Some((_, sum)) => *sum += value as f32,
            None => self.rows.push((key, value as f32)),
        }
    }

    /// Task Manager sums each engine type across processes, then shows the
    /// busiest type. This returns that number.
    fn busiest(&self) -> f32 {
        self.rows
            .iter()
            .map(|(_, sum)| *sum)
            .fold(0.0f32, f32::max)
            .clamp(0.0, 100.0)
    }
}

/// A physical adapter; one that was down at the last scan is not read each tick.
#[derive(Clone, Copy)]
struct NetLink {
    luid: u64,
    up: bool,
}

/// What an adapter's byte counters read at the last tick.
#[derive(Clone, Copy)]
struct NetSeen {
    luid: u64,
    received: u64,
    sent: u64,
}

pub struct Sampler {
    query: Query,
    cpu_time: Option<Counter>,
    cpu_performance: Option<Counter>,
    disk_idle: Option<Counter>,
    gpu_engine: Option<Counter>,
    thermal_zone: Option<Counter>,
    has_temp: bool,
    /// The physical network adapters, rescanned every `NET_LINKS_EVERY` ticks.
    net_links: Vec<NetLink>,
    net_seen: Vec<NetSeen>,
    net_prev: Vec<NetSeen>,
    net_at: Option<Instant>,
    base_mhz: u32,
    gpu_luid: Option<AdapterLuid>,
    npu_luid: Option<AdapterLuid>,
    gpu_sums: EngineSums,
    npu_sums: EngineSums,
    /// A rate counter needs two collects. The first tick yields no value.
    primed: bool,
    ticks: u64,
    volumes: Vec<Volume>,
}

impl Sampler {
    pub fn new() -> Result<Self, PdhError> {
        let adapters = wddm::enumerate();
        let gpu_luid = wddm::primary_gpu(&adapters).map(|a| a.luid);
        let npu_luid = wddm::npu(&adapters).map(|a| a.luid);
        let mut sampler = Self {
            query: Query::new()?,
            cpu_time: None,
            cpu_performance: None,
            disk_idle: None,
            gpu_engine: None,
            thermal_zone: None,
            has_temp: false,
            net_links: Vec::new(),
            net_seen: Vec::new(),
            net_prev: Vec::new(),
            net_at: None,
            base_mhz: base_clock_mhz(),
            gpu_luid,
            npu_luid,
            gpu_sums: EngineSums::default(),
            npu_sums: EngineSums::default(),
            primed: false,
            ticks: 0,
            volumes: Vec::new(),
        };
        sampler.open_counters();
        Ok(sampler)
    }

    fn open_counters(&mut self) {
        self.cpu_time = self.query.add_optional(CPU_TIME);
        self.cpu_performance = self.query.add_optional(CPU_PERFORMANCE);
        self.disk_idle = self.query.add_optional(DISK_IDLE);
        self.gpu_engine = self.query.add_optional(GPU_ENGINE);
        self.thermal_zone = self.query.add_optional(THERMAL_ZONE);
        let _ = self.query.collect();
        self.primed = false;
        // A gauge reads after one collect, so presence is known before the first tick.
        self.has_temp = self.read_temp().is_some();
        self.net_seen.clear();
        self.net_prev.clear();
        self.net_at = None;
        read_net_links(&mut self.net_links);
    }

    /// Rebuild the query from scratch. Sleep and resume invalidates PDH
    /// state, so `WM_POWERBROADCAST` calls this.
    pub fn rebuild(&mut self) -> Result<(), PdhError> {
        let adapters = wddm::enumerate();
        self.gpu_luid = wddm::primary_gpu(&adapters).map(|a| a.luid);
        self.npu_luid = wddm::npu(&adapters).map(|a| a.luid);
        self.cpu_time = None;
        self.cpu_performance = None;
        self.disk_idle = None;
        self.gpu_engine = None;
        self.thermal_zone = None;
        self.query = Query::new()?;
        self.open_counters();
        Ok(())
    }

    /// True when the machine has an NPU, so the UI draws the NPU row
    /// (GLINT-NPU-OPTIONAL).
    pub fn has_npu(&self) -> bool {
        self.npu_luid.is_some()
    }

    /// True when a graphics adapter was found at startup.
    ///
    /// The layout asks this rather than `metrics.gpu.is_some()`, which is
    /// `None` on the unprimed first tick and would open the window one row
    /// short before growing a second later.
    pub fn has_gpu(&self) -> bool {
        self.gpu_luid.is_some()
    }

    /// True when the firmware exposes a thermal zone, so the UI offers the Temp row.
    pub fn has_temp(&self) -> bool {
        self.has_temp
    }

    /// True when a physical adapter exists, so the UI offers the Network row.
    pub fn has_network(&self) -> bool {
        !self.net_links.is_empty()
    }

    /// Sample every counter once.
    ///
    /// The first call after `new` or `rebuild` returns metrics with zeros for
    /// the rate rows, because a PDH rate needs two collects.
    pub fn tick(&mut self) -> Metrics {
        let _ = self.query.collect();
        let first = !self.primed;
        self.primed = true;
        self.ticks += 1;

        let mut metrics = Metrics::default();
        read_memory(&mut metrics);

        if !first {
            metrics.cpu = self
                .cpu_time
                .as_ref()
                .and_then(|counter| counter.value().ok())
                .unwrap_or(0.0)
                .clamp(0.0, 100.0) as f32;

            // `% Processor Performance` must stay uncapped: above the base
            // clock it exceeds 100, and that excess is the boost (GLINT-CPU-BOOST).
            let performance = self
                .cpu_performance
                .as_ref()
                .and_then(|counter| counter.value_nocap().ok())
                .unwrap_or(0.0)
                .max(0.0);
            metrics.cpu_ratio = (performance / 100.0) as f32;
            metrics.cpu_ghz = (f64::from(self.base_mhz) * performance / 100_000.0) as f32;

            let idle = self
                .disk_idle
                .as_ref()
                .and_then(|counter| counter.value().ok())
                .unwrap_or(100.0);
            metrics.disk = (100.0 - idle).clamp(0.0, 100.0) as f32;

            self.read_engines(&mut metrics);
        }
        if self.has_temp {
            metrics.temp = self.read_temp();
        }
        if self.ticks % NET_LINKS_EVERY == 0 {
            read_net_links(&mut self.net_links);
        }
        // Every tick, the first included, so the next one has a baseline.
        self.read_network(&mut metrics, first);

        // Read disk space on the first tick, then every 30 s.
        if self.volumes.is_empty() || self.ticks % DISK_SPACE_EVERY == 1 {
            read_volumes(&mut self.volumes);
        }
        metrics.volumes = self.volumes.clone();
        metrics
    }

    /// The hottest zone in Celsius; the counter reports kelvin.
    fn read_temp(&mut self) -> Option<f32> {
        let counter = self.thermal_zone.as_mut()?;
        let mut hottest = 0.0f64;
        counter
            .for_each_instance(|_, kelvin| hottest = hottest.max(kelvin))
            .ok()?;
        // A zone that reports 0 K has no sensor behind it.
        (hottest > 0.0).then(|| (hottest - 273.15).clamp(0.0, 100.0) as f32)
    }

    /// Busiest adapter wins; summing would count a VPN's traffic twice (GLINT-NETWORK-METRIC).
    fn read_network(&mut self, metrics: &mut Metrics, first: bool) {
        use windows::Win32::NetworkManagement::IpHelper::{GetIfEntry2, MIB_IF_ROW2};
        use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;

        let now = Instant::now();
        let seconds = self
            .net_at
            .map_or(0.0, |before| now.duration_since(before).as_secs_f64());
        self.net_at = Some(now);

        // Swap rather than clone, so a steady tick allocates nothing.
        std::mem::swap(&mut self.net_prev, &mut self.net_seen);
        self.net_seen.clear();
        let mut busiest = 0.0f64;
        for &NetLink { luid, up } in &self.net_links {
            if !up {
                continue;
            }
            let mut row = MIB_IF_ROW2::default();
            row.InterfaceLuid.Value = luid;
            if unsafe { GetIfEntry2(&mut row) }.0 != 0 || row.OperStatus != IfOperStatusUp {
                continue;
            }
            self.net_seen.push(NetSeen {
                luid,
                received: row.InOctets,
                sent: row.OutOctets,
            });
            let Some(last) = self.net_prev.iter().find(|last| last.luid == luid) else {
                continue;
            };
            if seconds <= 0.0 {
                continue;
            }
            // Each direction against its own link speed, in case the link is asymmetric.
            let share = |bytes: u64, speed: u64| {
                if speed == 0 {
                    0.0
                } else {
                    bytes as f64 * 8.0 / seconds / speed as f64 * 100.0
                }
            };
            let received = share(
                row.InOctets.saturating_sub(last.received),
                row.ReceiveLinkSpeed,
            );
            let sent = share(
                row.OutOctets.saturating_sub(last.sent),
                row.TransmitLinkSpeed,
            );
            busiest = busiest.max(received).max(sent);
        }

        if !first && !self.net_links.is_empty() {
            metrics.network = Some(busiest.clamp(0.0, 100.0) as f32);
        }
    }

    /// Sum `GPU Engine` utilization for the two adapters that the UI shows.
    fn read_engines(&mut self, metrics: &mut Metrics) {
        let Some(counter) = self.gpu_engine.as_mut() else {
            return;
        };
        self.gpu_sums.clear();
        self.npu_sums.clear();
        let gpu_luid = self.gpu_luid;
        let npu_luid = self.npu_luid;
        let gpu_sums = &mut self.gpu_sums;
        let npu_sums = &mut self.npu_sums;

        let read = counter.for_each_instance(|name, value| {
            let Ok(instance) = parse_engine_instance(name) else {
                return;
            };
            if Some(instance.luid) == gpu_luid {
                gpu_sums.add(instance.engtype, value);
            } else if Some(instance.luid) == npu_luid {
                npu_sums.add(instance.engtype, value);
            }
        });
        // A failed read leaves both rows absent rather than stale. The GPU
        // row then reads `--`, which is honest; a frozen number is not.
        if read.is_err() {
            return;
        }

        if gpu_luid.is_some() {
            metrics.gpu = Some(self.gpu_sums.busiest());
        }
        if npu_luid.is_some() {
            metrics.npu = Some(self.npu_sums.busiest());
        }
    }
}

/// Physical adapters only: the hardware flag drops filter-layer duplicates and virtual switches.
fn read_net_links(out: &mut Vec<NetLink>) {
    use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};
    use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;

    out.clear();
    let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
    if unsafe { GetIfTable2(&mut table) }.0 != 0 || table.is_null() {
        return;
    }
    let rows = unsafe {
        std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize)
    };
    for row in rows {
        const HARDWARE: u8 = 1;
        if row.InterfaceAndOperStatusFlags._bitfield & HARDWARE != 0 {
            out.push(NetLink {
                luid: unsafe { row.InterfaceLuid.Value },
                up: row.OperStatus == IfOperStatusUp,
            });
        }
    }
    unsafe { FreeMibTable(table.cast()) };
}

/// Read the CPU base clock in MHz.
///
/// Spike 1 compared both sources on an AMD Ryzen AI 9 HX PRO 370: the
/// registry reports 1996 and `CallNtPowerInformation` reports 2000, and the
/// true base clock is 2.0 GHz. Neither reports the 5.1 GHz boost ceiling.
/// The registry is the primary source, as the plan chose.
fn base_clock_mhz() -> u32 {
    registry_mhz().or_else(power_max_mhz).unwrap_or(0)
}

fn registry_mhz() -> Option<u32> {
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
    if status.is_ok() && value > 0 {
        Some(value)
    } else {
        None
    }
}

fn power_max_mhz() -> Option<u32> {
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
    buffer.first().map(|first| first.MaxMhz).filter(|mhz| *mhz > 0)
}

fn read_memory(metrics: &mut Metrics) {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    let mut status = MEMORYSTATUSEX {
        dwLength: size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    if unsafe { GlobalMemoryStatusEx(&mut status) }.is_err() {
        return;
    }
    metrics.memory_total_bytes = status.ullTotalPhys;
    metrics.memory_used_bytes = status.ullTotalPhys.saturating_sub(status.ullAvailPhys);
    metrics.memory = f32::from(status.dwMemoryLoad as u16).clamp(0.0, 100.0);
}

/// `DRIVE_FIXED` from `winbase.h`. The `windows` crate does not bind the
/// return values of `GetDriveTypeW`.
const DRIVE_FIXED: u32 = 3;

/// List every fixed drive and its space. Called once every 30 s.
fn read_volumes(out: &mut Vec<Volume>) {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives,
    };

    out.clear();
    let mask = unsafe { GetLogicalDrives() };
    for index in 0..26u32 {
        if mask & (1 << index) == 0 {
            continue;
        }
        let letter = char::from(b'A' + index as u8);
        // `C:\` with the NUL that Win32 needs.
        let root: [u16; 4] = [letter as u16, u16::from(b':'), u16::from(b'\\'), 0];
        if unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) } != DRIVE_FIXED {
            continue;
        }
        let mut total = 0u64;
        let mut free = 0u64;
        let read = unsafe {
            GetDiskFreeSpaceExW(
                PCWSTR(root.as_ptr()),
                None,
                Some(&mut total),
                Some(&mut free),
            )
        };
        if read.is_err() || total == 0 {
            continue;
        }
        out.push(Volume {
            label: format!("{letter}:"),
            used_bytes: total.saturating_sub(free),
            total_bytes: total,
        });
    }
}
