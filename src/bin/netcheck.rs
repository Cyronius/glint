//! Per-adapter PDH network rates against link speed; `iftable` dumps `GetIfTable2` and its cost.

use std::time::{Duration, Instant};

use glint::pdh::Query;

const RECEIVED: &str = r"\Network Interface(*)\Bytes Received/sec";
const SENT: &str = r"\Network Interface(*)\Bytes Sent/sec";
const TOTAL: &str = r"\Network Interface(*)\Bytes Total/sec";
const BANDWIDTH: &str = r"\Network Interface(*)\Current Bandwidth";

#[derive(Default, Clone)]
struct Adapter {
    name: String,
    received: f64,
    sent: f64,
    total: f64,
    bandwidth: f64,
}

fn slot<'a>(adapters: &'a mut Vec<Adapter>, name: &str) -> &'a mut Adapter {
    if let Some(at) = adapters.iter().position(|a| a.name == name) {
        return &mut adapters[at];
    }
    adapters.push(Adapter {
        name: name.to_string(),
        ..Default::default()
    });
    adapters.last_mut().unwrap()
}

fn if_table() {
    use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};

    for round in 0..3 {
        let started = Instant::now();
        let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        let status = unsafe { GetIfTable2(&mut table) };
        let took = started.elapsed();
        if status.0 != 0 || table.is_null() {
            println!("GetIfTable2 failed: {}", status.0);
            return;
        }
        let count = unsafe { (*table).NumEntries } as usize;
        let rows = unsafe { std::slice::from_raw_parts((*table).Table.as_ptr(), count) };
        println!(
            "round {round}: {count} interfaces, GetIfTable2 {:.3} ms",
            took.as_secs_f64() * 1000.0
        );
        if round == 0 {
            for row in rows {
                let end = row.Description.iter().position(|c| *c == 0).unwrap_or(0);
                let name = String::from_utf16_lossy(&row.Description[..end]);
                println!(
                    "  type {:>3}  oper {}  hw {}  rx {:>6} tx {:>6} Mbit/s  {}",
                    row.Type,
                    row.OperStatus.0,
                    row.InterfaceAndOperStatusFlags._bitfield & 1,
                    row.ReceiveLinkSpeed / 1_000_000,
                    row.TransmitLinkSpeed / 1_000_000,
                    name
                );
            }
        }
        unsafe { FreeMibTable(table.cast()) };
    }
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("iftable") {
        return if_table();
    }
    let rounds: u32 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(10);

    let query = Query::new().expect("PdhOpenQuery");
    let mut received = query.add(RECEIVED).expect("add received");
    let mut sent = query.add(SENT).expect("add sent");
    let mut total = query.add(TOTAL).expect("add total");
    let mut bandwidth = query.add(BANDWIDTH).expect("add bandwidth");
    let _ = query.collect();

    for round in 1..=rounds {
        std::thread::sleep(Duration::from_millis(1000));
        let started = Instant::now();
        let _ = query.collect();
        let collected = started.elapsed();

        let mut adapters: Vec<Adapter> = Vec::new();
        let _ = received.for_each_instance(|n, v| slot(&mut adapters, n).received = v);
        let _ = sent.for_each_instance(|n, v| slot(&mut adapters, n).sent = v);
        let _ = total.for_each_instance(|n, v| slot(&mut adapters, n).total = v);
        let _ = bandwidth.for_each_instance(|n, v| slot(&mut adapters, n).bandwidth = v);
        let read = started.elapsed() - collected;

        println!(
            "round {round}: {} adapters, collect {:.3} ms, four reads {:.3} ms",
            adapters.len(),
            collected.as_secs_f64() * 1000.0,
            read.as_secs_f64() * 1000.0
        );
        for a in &adapters {
            let pct = |bytes: f64| {
                if a.bandwidth > 0.0 {
                    bytes * 8.0 / a.bandwidth * 100.0
                } else {
                    0.0
                }
            };
            println!(
                "  {:<48} rx {:>8.2}  tx {:>8.2} Mbit/s  link {:>7.0} Mbit/s  total% {:>6.2}  max% {:>6.2}",
                a.name,
                a.received * 8.0 / 1e6,
                a.sent * 8.0 / 1e6,
                a.bandwidth / 1e6,
                pct(a.total),
                pct(a.received.max(a.sent)),
            );
        }
    }
}
