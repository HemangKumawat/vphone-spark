//! vphone-fleet: makes, runs, parks and snapshots the phones of one host (docs/DENSITY.md).

mod api;
mod client;
mod config;
mod diag;
mod events;
mod fleet;
mod gateway;
mod host;
mod journal;
mod mcp;
mod phone;
mod screen;
mod server;
#[cfg(test)]
mod tests;
mod ws;

use std::env;
use std::io::{self, Write as _};
use std::path::Path;
use std::process::ExitCode;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;
use vphone_api_proxy::log;

use crate::client::Client;
use crate::config::{Config, Feature, Paths};
use crate::fleet::{Fleet, stop_phone};
use crate::phone::CreateSpec;

const USAGE: &str = "\
usage: vphone-fleet COMMAND

  golden add DIR [--name NAME]   make the stopped VM folder DIR a golden (moved to FLEET/golden/NAME)
  create ID... --from GOLDEN [--profile NAME]
                                 make phones from a golden: reflinks of its state, their own
                                 serial, MAC, vsock CID and API port (new ECID: [identity] rotate_ecid)
  start ID...|--all              start phones (tools/phone-up.sh, detached; console in runs/ID/)
  stop ID...|--all               stop phones (SIGTERM, SIGKILL after 10 s)
  up [ID...]                     start phones (default all) and wait until each answers /v1/health
  down                           stop every phone
  list                           every phone, its state, CID and API port
  snapshot ID NAME               cold snapshot of a stopped phone (reflinks, in its own folder)
  restore ID NAME                put a stopped phone back to one of its own snapshots
  reset ID --confirm ID          fresh state from its golden; keeps identity and snapshots
  delete ID --confirm ID         delete a stopped phone, snapshots included
  features                       the feature switches, each with its value and source
  serve [--listen HOST:PORT]     the control API (default 127.0.0.1:8760) and the 1 s loop:
                                 health, auto-restart, parking, CPU priority
  doctor [ID] [--logs]           every check with a hint per failure (a running serve's /diag,
                                 else checked from here); --logs adds each phone's log tails
  mcp                            the fleet as MCP tools for AI agents, on stdin/stdout (talks
                                 to the running serve: VPHONE_FLEET_URL, VPHONE_FLEET_TOKEN)

FLEET (default <checkout>/vms) holds golden/ and phones/; runs/ holds run-time files. The control
API's token is VPHONE_FLEET_TOKEN, else a new one written to runs/fleet-token (mode 0600).
The API, with examples: docs/AUTOMATION.md and docs/openapi.yaml.
Feature switches: config/profiles.toml [features] < VPHONE_FEATURE_<NAME>=0|1 < PUT /features/<name>.";

/// The default control API address: loopback, next to vphone-cli's 8765 example port.
const LISTEN: &str = "127.0.0.1:8760";
/// Lines of each log `doctor --logs` prints.
const DOCTOR_TAIL: usize = 40;

fn say(line: &str) {
    // stdout may be a closed pipe (`list | head`): not worth a panic.
    let _ = writeln!(io::stdout(), "{line}");
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match run(&args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            log!("vphone-fleet: {e}");
            ExitCode::from(2)
        }
    }
}

/// `--flag [value]` options, in order.
type Opts<'arg> = Vec<(&'arg str, Option<&'arg str>)>;

/// Splits `args` into positionals and options; `takes` lists the flags that take a value.
fn options<'arg>(
    args: &'arg [String],
    takes: &[&str],
) -> Result<(Vec<&'arg str>, Opts<'arg>), String> {
    let (mut pos, mut opts) = (Vec::new(), Vec::new());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a.starts_with("--") {
            let value = if takes.contains(&a.as_str()) {
                Some(it.next().ok_or(format!("{a} needs a value"))?.as_str())
            } else {
                None
            };
            opts.push((a.as_str(), value));
        } else {
            pos.push(a.as_str());
        }
    }
    Ok((pos, opts))
}

fn opt<'arg>(opts: &[(&str, Option<&'arg str>)], name: &str) -> Option<&'arg str> {
    opts.iter().find(|(n, _)| *n == name).and_then(|(_, v)| *v)
}

fn flag(opts: &[(&str, Option<&str>)], name: &str) -> bool {
    opts.iter().any(|(n, _)| *n == name)
}

/// Prints each outcome; whether all succeeded.
fn report(results: &[(String, Result<String, String>)]) -> bool {
    for (id, r) in results {
        match r {
            Ok(m) => say(m),
            Err(e) => log!("vphone-fleet: {id}: {e}"),
        }
    }
    results.iter().all(|(_, r)| r.is_ok())
}

fn run(args: &[String]) -> Result<bool, String> {
    let Some((cmd, rest)) = args.split_first() else {
        say(USAGE);
        return Ok(false);
    };
    if matches!(cmd.as_str(), "-h" | "--help" | "help") {
        say(USAGE);
        return Ok(true);
    }
    let paths = Paths::find()?;
    let config = Config::load(&paths)?;
    let (pos, opts) = options(
        rest,
        &["--name", "--from", "--profile", "--confirm", "--listen"],
    )?;
    match cmd.as_str() {
        "golden" | "create" | "snapshot" | "restore" | "reset" | "delete" => {
            files(cmd, &paths, &config, &pos, &opts)
        }
        "start" | "up" | "stop" | "down" | "list" => phones(cmd, paths, config, &pos, &opts),
        "features" => {
            say(config.features.table().trim_end());
            say("(a running `vphone-fleet serve` may differ: GET /features shows its live values)");
            Ok(true)
        }
        "serve" => {
            server::serve(&paths, config, opt(&opts, "--listen").unwrap_or(LISTEN)).map(|()| true)
        }
        "doctor" => Ok(doctor(
            &paths,
            config,
            pos.first().copied(),
            flag(&opts, "--logs").then_some(DOCTOR_TAIL),
        )),
        "mcp" => {
            let client = Client::find(&paths, LISTEN);
            // A running serve's live switch (PUT /features/mcp) wins over the file's.
            let live = client
                .json("GET", "/features", "")
                .ok()
                .and_then(|(_, b)| serde_json::from_str::<Value>(&b).ok())
                .and_then(|v| v["mcp"]["on"].as_bool());
            if !live.unwrap_or(config.features.on(Feature::Mcp)) {
                return Err("feature mcp is off: PUT /features/mcp {\"on\": true}, or mcp = true in config/profiles.toml [features]".to_owned());
            }
            mcp::run(io::stdin().lock(), io::stdout().lock(), &client).map(|()| true)
        }
        other => Err(format!("unknown command {other}\n\n{USAGE}")),
    }
}

/// The commands on phones' files: golden add, create, snapshot, restore, reset, delete.
fn files(
    cmd: &str,
    paths: &Paths,
    config: &Config,
    pos: &[&str],
    opts: &[(&str, Option<&str>)],
) -> Result<bool, String> {
    let usage = |u: &str| Err(format!("{u}\n\n{USAGE}"));
    let line = match (cmd, pos) {
        ("golden", ["add", dir]) => phone::golden_add(paths, Path::new(dir), opt(opts, "--name"))?,
        ("golden", _) => return usage("golden add DIR [--name NAME]"),
        ("create", ids) if !ids.is_empty() => {
            let spec = CreateSpec {
                golden: opt(opts, "--from").ok_or("create needs --from GOLDEN")?,
                profile: opt(opts, "--profile"),
            };
            let results: Vec<_> = ids
                .iter()
                .map(|id| ((*id).to_owned(), phone::create(paths, config, id, &spec)))
                .collect();
            return Ok(report(&results));
        }
        ("create", _) => return usage("create ID... --from GOLDEN"),
        ("snapshot", [id, name]) => phone::snapshot(paths, id, name)?,
        ("restore", [id, name]) => phone::restore(paths, id, name)?,
        ("reset", [id]) => phone::reset(paths, id, opt(opts, "--confirm"))?,
        ("delete", [id]) => phone::delete(paths, id, opt(opts, "--confirm"))?,
        ("snapshot" | "restore", _) => return usage(&format!("{cmd} ID NAME")),
        _ => return usage(&format!("{cmd} ID --confirm ID")),
    };
    say(&line);
    Ok(true)
}

/// The commands on running phones: start, up, stop, down, list.
fn phones(
    cmd: &str,
    paths: Paths,
    config: Config,
    pos: &[&str],
    opts: &[(&str, Option<&str>)],
) -> Result<bool, String> {
    let all = flag(opts, "--all") || cmd == "up" || cmd == "down";
    if pos.is_empty() && !all && cmd != "list" {
        return Err(format!("{cmd} ID... or {cmd} --all"));
    }
    let ids: Vec<String> = if pos.is_empty() {
        phone::list(&paths).into_iter().map(|p| p.id).collect()
    } else {
        pos.iter().map(|s| (*s).to_owned()).collect()
    };
    if cmd == "stop" || cmd == "down" {
        let results: Vec<_> = ids
            .into_iter()
            .map(|id| {
                let r = stop_phone(&paths, &id);
                (id, r)
            })
            .collect();
        return Ok(report(&results));
    }
    let mut fleet = Fleet::new(paths, config);
    fleet.adopt();
    if cmd == "list" {
        for line in fleet.list_json() {
            say(&line);
        }
        return Ok(true);
    }
    let results: Vec<_> = ids.iter().map(|id| (id.clone(), fleet.start(id))).collect();
    let ok = report(&results);
    Ok(if cmd == "up" {
        wait_healthy(&fleet, &ids) && ok
    } else {
        ok
    })
}

/// `vphone-fleet doctor`: the running serve's report, else one made here (without the serve's
/// restart counts and failure history), with the last `tail` lines of each log if asked;
/// whether nothing failed.
fn doctor(paths: &Paths, config: Config, id: Option<&str>, tail: Option<usize>) -> bool {
    let path = id.map_or_else(|| "/diag".to_owned(), |id| format!("/phones/{id}/diag"));
    let from_serve = Client::find(paths, LISTEN)
        .json("GET", &path, "")
        .and_then(|(code, body)| {
            serde_json::from_str::<Value>(&body).map_err(|e| format!("{path}: {code}: {e}"))
        });
    let report = from_serve.unwrap_or_else(|e| {
        say(&format!(
            "(no answer from vphone-fleet serve: {e}; checking from here)"
        ));
        let mut fleet = Fleet::new(paths.clone(), config);
        fleet.adopt();
        let fleet = Mutex::new(fleet);
        id.map_or_else(|| diag::report(&fleet), |id| diag::phone_report(&fleet, id))
    });
    let ok = diag::print(&report, say);
    if let Some(tail) = tail {
        let ids: Vec<String> = id.map_or_else(
            || phone::list(paths).into_iter().map(|p| p.id).collect(),
            |id| vec![id.to_owned()],
        );
        for id in ids {
            match diag::logs(paths, &id, tail) {
                Ok(v) => {
                    for f in v["files"].as_array().into_iter().flatten() {
                        say(&format!("--- {id} {}", f["file"].as_str().unwrap_or("?")));
                        say(f["text"].as_str().or(f["error"].as_str()).unwrap_or(""));
                    }
                }
                Err(e) => say(&format!("--- {id}: {e}")),
            }
        }
    }
    ok
}

/// Waits up to `boot_secs` for each phone's `/v1/health`; prints each outcome.
fn wait_healthy(fleet: &Fleet, ids: &[String]) -> bool {
    let start = Instant::now();
    let limit = Duration::from_secs(fleet.config.tunables.boot_secs);
    let mut waiting: Vec<&String> = ids.iter().collect();
    while !waiting.is_empty() && start.elapsed() < limit {
        waiting.retain(|id| {
            let up = fleet.upstream_of(id).is_some_and(|u| host::healthy(&u));
            if up {
                say(&format!("{id} healthy t={}s", start.elapsed().as_secs()));
            }
            !up
        });
        if !waiting.is_empty() {
            thread::sleep(Duration::from_secs(1));
        }
    }
    for id in &waiting {
        log!("vphone-fleet: {id} did not answer /v1/health within {limit:?}");
    }
    say(&format!(
        "{} of {} phones healthy",
        ids.len() - waiting.len(),
        ids.len()
    ));
    waiting.is_empty()
}
