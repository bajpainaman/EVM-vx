use crate::workload::Workload;
use std::path::PathBuf;

pub const USAGE: &str = "usage: evmvx-test statetest|btest <path>... [--show N]\n\
       evmvx-test bench <path>... [--reps N]\n\
       evmvx-test pexec [--txs N] [--threads N] [--contracts N] [--reps N] [--workload NAME] [--json]\n\
       evmvx-test pfuzz [--blocks N] [--txs N] [--seed N] [--threads N]";

pub struct Options {
    pub mode: String,
    pub show: usize,
    pub reps: u32,
    pub txs: usize,
    pub threads: usize,
    pub contracts: u64,
    pub blocks: usize,
    pub seed: u64,
    pub workload: Workload,
    pub json: bool,
    pub paths: Vec<PathBuf>,
}

pub fn parse(arguments: &[String]) -> Result<Options, String> {
    let mode = arguments.first().ok_or("missing execution mode")?.as_str();
    if !matches!(mode, "statetest" | "btest" | "bench" | "pexec" | "pfuzz") {
        return Err(format!("unknown execution mode {mode:?}"));
    }
    let mut options = Options {
        mode: mode.into(),
        show: 20,
        reps: 1,
        txs: 2000,
        threads: 8,
        contracts: 2000,
        blocks: 200,
        seed: 1,
        workload: Workload::Storage,
        json: false,
        paths: Vec::new(),
    };
    let mut iterator = arguments[1..].iter();
    while let Some(argument) = iterator.next() {
        if argument == "--json" && mode == "pexec" {
            options.json = true;
            continue;
        }
        if !argument.starts_with('-') {
            if matches!(mode, "pexec" | "pfuzz") {
                return Err(format!("unexpected argument {argument:?} for {mode}"));
            }
            options.paths.push(argument.into());
            continue;
        }
        let allowed = match argument.as_str() {
            "--show" => matches!(mode, "statetest" | "btest"),
            "--reps" => matches!(mode, "bench" | "pexec"),
            "--txs" | "--threads" => matches!(mode, "pexec" | "pfuzz"),
            "--contracts" | "--workload" => mode == "pexec",
            "--blocks" | "--seed" => mode == "pfuzz",
            _ => false,
        };
        if !allowed {
            return Err(format!(
                "unknown or unsupported option {argument:?} for {mode}"
            ));
        }
        let value = iterator
            .next()
            .ok_or_else(|| format!("{argument} requires a value"))?;
        if argument == "--workload" {
            options.workload = value
                .parse()
                .map_err(|error| format!("--workload: {error}"))?;
            continue;
        }
        let number: u64 = value
            .parse()
            .map_err(|_| format!("{argument} requires an unsigned integer, got {value:?}"))?;
        if number == 0 && !matches!(argument.as_str(), "--seed" | "--show") {
            return Err(format!("{argument} must be greater than zero"));
        }
        let size = usize::try_from(number)
            .map_err(|_| format!("{argument} exceeds this platform's size limit"))?;
        match argument.as_str() {
            "--show" => options.show = size,
            "--reps" => {
                options.reps =
                    u32::try_from(number).map_err(|_| "--reps exceeds u32".to_string())?
            }
            "--txs" => options.txs = size,
            "--threads" => options.threads = size,
            "--contracts" => options.contracts = number,
            "--blocks" => options.blocks = size,
            "--seed" => options.seed = number,
            _ => unreachable!("option was checked above"),
        }
    }
    if mode == "pfuzz" && options.txs < 2 {
        return Err("--txs must be at least 2 for pfuzz".into());
    }
    if !matches!(mode, "pexec" | "pfuzz") && options.paths.is_empty() {
        return Err("at least one fixture path is required".into());
    }
    Ok(options)
}
