use opurom_core::{measure_speedup, SpeedupMeasurement};
use std::env;
use std::str::FromStr;
use std::thread::available_parallelism;

fn usage() -> ! {
    eprintln!("usage: parallel_speedup [tasks] [iterations_per_task] [workers]");
    std::process::exit(2);
}

fn parse<T: FromStr>(value: Option<&String>, default: T) -> T {
    match value {
        None => default,
        Some(text) => text.parse().unwrap_or_else(|_| usage()),
    }
}

fn checksum(results: &[u64]) -> u64 {
    results.iter().fold(0, |acc, &value| acc ^ value)
}

fn print_measurement(measurement: &SpeedupMeasurement) {
    let one_ms = measurement.one_worker.as_secs_f64() * 1_000.0;
    let many_ms = measurement.many_workers.as_secs_f64() * 1_000.0;
    let one_checksum = checksum(&measurement.one_results);
    let many_checksum = checksum(&measurement.many_results);

    println!("OPUROM R9 MEASURED parallel speedup (std::thread, CPU-bound integer workload)");
    println!(
        "tasks={} iterations_per_task={} n_workers={}",
        measurement.tasks, measurement.iterations, measurement.workers
    );
    println!("| run | workers | Instant elapsed (ms) | result checksum |");
    println!("| --- | ---: | ---: | ---: |");
    println!("| 1-worker | 1 | {one_ms:.3} | {one_checksum} |");
    println!(
        "| N-worker | {} | {many_ms:.3} | {many_checksum} |",
        measurement.workers
    );
    println!(
        "measured ratio = {one_ms:.3} ms / {many_ms:.3} ms = {:.3}x",
        measurement.ratio
    );

    if one_checksum != many_checksum {
        eprintln!("error: 1-worker and N-worker results differ");
        std::process::exit(1);
    }
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.len() > 3 {
        usage();
    }
    let tasks = parse(args.get(0), 16usize);
    let iterations = parse(args.get(1), 300_000u64);
    let default_workers = available_parallelism().map_or(2, |value| value.get().clamp(2, 8));
    let workers = parse(args.get(2), default_workers);

    match measure_speedup(workers, tasks, iterations) {
        Ok(measurement) => print_measurement(&measurement),
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(1);
        }
    }
}
