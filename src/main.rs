use opurom_core::{Event, Runtime};
use std::io::{self, BufRead};
use std::process::ExitCode;

fn main() -> ExitCode {
    let Some(path) = std::env::args_os().nth(1) else {
        eprintln!("usage: opurom-core <app.opuir.json>");
        return ExitCode::from(2);
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("error: cannot read {}: {error}", path.to_string_lossy());
            return ExitCode::from(1);
        }
    };
    let mut runtime = match Runtime::from_ir_json(&text) {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(1);
        }
    };
    print_state(&runtime);

    for line in io::stdin().lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                println!("error: {error}");
                break;
            }
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match line {
            "quit" => break,
            "state" => print_state(&runtime),
            _ => match parse_command(line) {
                Ok(event) => match runtime.dispatch(event) {
                    Ok(patches) => match serde_json::to_string(&patches) {
                        Ok(json) => println!("patches {json} version {}", runtime.version()),
                        Err(error) => println!("error: {error}"),
                    },
                    Err(error) => println!("error: {error}"),
                },
                Err(message) => println!("error: {message}"),
            },
        }
    }
    ExitCode::SUCCESS
}

fn print_state(runtime: &Runtime) {
    match serde_json::to_string(&runtime.snapshot()) {
        Ok(json) => println!("state {json} version {}", runtime.version()),
        Err(error) => println!("error: {error}"),
    }
}

fn parse_command(line: &str) -> Result<Event, String> {
    let mut parts = line.splitn(2, ' ');
    let command = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("").trim();
    match command {
        "tap" => {
            let node_id = rest
                .parse::<u32>()
                .map_err(|error| format!("tap expects `tap <id>`: {error}"))?;
            Ok(Event::Tap { node_id })
        }
        "input" => {
            let mut parts = rest.splitn(2, ' ');
            let node_id = parts
                .next()
                .unwrap_or("")
                .parse::<u32>()
                .map_err(|error| format!("input expects `input <id> <text>`: {error}"))?;
            let text = parts
                .next()
                .ok_or_else(|| "input expects `input <id> <text>`".to_owned())?;
            Ok(Event::Input {
                node_id,
                text: text.to_owned(),
            })
        }
        _ => Err(format!(
            "unknown command {command:?}; expected tap, input, state or quit"
        )),
    }
}
