//! What does `% Processor Utility` do when the clock scales?
//!
//! Three counters side by side:
//!
//! - `% Processor Time`: the fraction of wall time the cores were not idle.
//!   It ignores the clock entirely.
//! - `% Processor Performance`: the current clock as a percent of the base
//!   clock. 100 means the base clock, 200 means twice it.
//! - `% Processor Utility`: the work done, measured against the base clock.
//!
//! If utility is normalised against the base clock, then
//! `utility == time * performance / 100`, and utility rises above 100 when
//! the clock boosts. If it were normalised against the boost ceiling, utility
//! would fall below time instead.
//!
//! Run `cargo run --release --bin cpucheck`. Load the CPU part way through to
//! see the relationship at more than one point.

use std::time::Duration;

use glint::pdh::Query;

const TIME: &str = r"\Processor Information(_Total)\% Processor Time";
const PERFORMANCE: &str = r"\Processor Information(_Total)\% Processor Performance";
const UTILITY: &str = r"\Processor Information(_Total)\% Processor Utility";

fn main() {
    let rounds: u32 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(12);

    let query = Query::new().expect("PdhOpenQuery");
    let time = query.add(TIME).expect("add time");
    let performance = query.add(PERFORMANCE).expect("add performance");
    let utility = query.add(UTILITY).expect("add utility");
    let _ = query.collect();

    // Keep a few cores busy for the second half of the run, so the clock
    // boosts and the three counters separate.
    let busy = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    for _ in 0..8 {
        let busy = busy.clone();
        std::thread::spawn(move || {
            let mut sink = 0u64;
            loop {
                if busy.load(std::sync::atomic::Ordering::Relaxed) {
                    for step in 0..2_000_000u64 {
                        sink = sink.wrapping_add(step).wrapping_mul(2_654_435_761);
                    }
                    std::hint::black_box(sink);
                } else {
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        });
    }

    println!(
        "{:>5}  {:>8}  {:>12}  {:>9}  {:>18}  {:>7}",
        "round", "time %", "performance %", "utility %", "time * perf / 100", "state"
    );
    for round in 1..=rounds {
        if round == rounds / 2 {
            busy.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        std::thread::sleep(Duration::from_millis(1000));
        let _ = query.collect();

        let time_value = time.value_nocap().unwrap_or(0.0);
        let performance_value = performance.value_nocap().unwrap_or(0.0);
        let utility_value = utility.value_nocap().unwrap_or(0.0);
        let predicted = time_value * performance_value / 100.0;
        let state = if busy.load(std::sync::atomic::Ordering::Relaxed) {
            "loaded"
        } else {
            "idle"
        };
        println!(
            "{round:>5}  {time_value:>8.1}  {performance_value:>13.1}  {utility_value:>9.1}  {predicted:>18.1}  {state:>7}"
        );
    }

    println!();
    println!("If utility tracks the last column, it is measured against the base clock.");
    println!("Then a faster clock RAISES the percent, and it can pass 100.");
}
