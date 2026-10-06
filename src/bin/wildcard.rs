//! Spike 1, Risk B, part two: does a PDH wildcard counter report instances
//! that appear after the counter was added?
//!
//! The plan assumed no, and planned a re-expand every 30 s. A re-expand costs
//! one bad sample, because a fresh rate counter needs two collects. This probe
//! settles whether the cost is needed at all.
//!
//! The test is decisive because it searches by process id. A process that
//! starts after the add has a process id that no earlier instance name holds.
//! The probe then asks two counters about it: one added before the process
//! started, and one added after.
//!
//! Run `cargo run --release --bin wildcard`.

use std::collections::BTreeSet;
use std::process::{Child, Command};
use std::time::Duration;

use glint::pdh::{Counter, Query};

const PATH: &str = r"\GPU Engine(*)\Utilization Percentage";

/// Read every instance name of a wildcard counter.
fn names(counter: &mut Counter) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    if let Err(error) = counter.for_each_instance(|name, _| {
        out.insert(name.to_string());
    }) {
        println!("  read failed: {error}");
    }
    out
}

/// Count the instance names that belong to one process id.
fn rows_for(set: &BTreeSet<String>, pid: u32) -> usize {
    let needle = format!("pid_{pid}_");
    set.iter().filter(|name| name.starts_with(&needle)).count()
}

/// Start a process that draws, so that it earns a `GPU Engine` instance.
fn start_drawing_process() -> Option<(Child, u32)> {
    for program in ["mspaint.exe", "notepad.exe", "explorer.exe"] {
        if let Ok(child) = Command::new(program).spawn() {
            let pid = child.id();
            println!("started {program}, pid {pid}");
            return Some((child, pid));
        }
    }
    None
}

fn main() {
    // The aged counter. It exists before the test process starts.
    let aged_query = Query::new().expect("PdhOpenQuery");
    let mut aged = aged_query.add(PATH).expect("add GPU Engine");
    let _ = aged_query.collect();
    std::thread::sleep(Duration::from_millis(1100));
    let _ = aged_query.collect();
    let baseline = names(&mut aged);
    println!("aged counter baseline: {} instances", baseline.len());

    let Some((mut child, pid)) = start_drawing_process() else {
        return println!("could not start a test process");
    };

    // Give the process time to create a device and to draw a few frames.
    for _ in 0..6 {
        std::thread::sleep(Duration::from_millis(1000));
        let _ = aged_query.collect();
    }

    // The fresh counter. It exists only after the test process started.
    let fresh_query = Query::new().expect("fresh PdhOpenQuery");
    let mut fresh = fresh_query.add(PATH).expect("fresh add");
    let _ = fresh_query.collect();
    std::thread::sleep(Duration::from_millis(1100));
    let _ = fresh_query.collect();
    let _ = aged_query.collect();

    let aged_now = names(&mut aged);
    let fresh_now = names(&mut fresh);
    let _ = child.kill();

    println!();
    println!("aged counter : {} instances", aged_now.len());
    println!("fresh counter: {} instances", fresh_now.len());
    println!(
        "rows for pid {pid}: aged {}, fresh {}",
        rows_for(&aged_now, pid),
        rows_for(&fresh_now, pid)
    );
    println!(
        "fresh-only: {}   aged-only: {}",
        fresh_now.difference(&aged_now).count(),
        aged_now.difference(&fresh_now).count()
    );

    println!();
    if rows_for(&fresh_now, pid) == 0 {
        println!("The test process earned no instance, so this run proves nothing.");
        println!("Run it again, or drive a real graphics load.");
    } else if rows_for(&aged_now, pid) > 0 {
        println!("The wildcard re-expands on its own. No periodic re-add is needed.");
    } else {
        println!("The wildcard is a snapshot from the time of the add.");
        println!("The sampler must re-create the counter to see new instances.");
    }
}
