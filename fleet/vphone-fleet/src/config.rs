//! `config/profiles.toml`'s `[identity]`, `[fleet]` and `[features]`, and the feature switches.

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use toml::{Table, Value};

/// Where a checkout keeps its tools, its config and its phones.
#[derive(Debug, Clone)]
pub(crate) struct Paths {
    /// The checkout: `tools/phone-up.sh`, `config/profiles.toml`.
    pub(crate) root: PathBuf,
    /// `FLEET`, default `<root>/vms`: `golden/` and `phones/`, one reflink filesystem.
    pub(crate) fleet: PathBuf,
    /// `<root>/runs`: each phone's run-time files (`tools/phone-up.sh` writes there too).
    pub(crate) runs: PathBuf,
}

impl Paths {
    /// The checkout holding this binary (`fleet/target/...`), else the one holding the current
    /// directory; `VPHONE_ROOT` overrides both.
    pub(crate) fn find() -> Result<Self, String> {
        let is_root = |d: &Path| d.join("tools/phone-up.sh").is_file();
        let root = if let Some(r) = env::var_os("VPHONE_ROOT") {
            PathBuf::from(r)
        } else {
            let exe = env::current_exe().ok();
            let cwd = env::current_dir().ok();
            exe.iter()
                .chain(cwd.iter())
                .flat_map(|p| p.ancestors())
                .find(|d| is_root(d))
                .map(Path::to_path_buf)
                .ok_or("no vphone checkout (tools/phone-up.sh) above this binary or the current directory: set VPHONE_ROOT")?
        };
        if !is_root(&root) {
            return Err(format!("{} has no tools/phone-up.sh", root.display()));
        }
        let fleet = env::var_os("FLEET").map_or_else(|| root.join("vms"), PathBuf::from);
        Ok(Self {
            runs: root.join("runs"),
            fleet,
            root,
        })
    }

    pub(crate) fn phone(&self, id: &str) -> PathBuf {
        self.fleet.join("phones").join(id)
    }

    pub(crate) fn run(&self, id: &str) -> PathBuf {
        self.runs.join(id)
    }

    pub(crate) fn profiles(&self) -> PathBuf {
        self.root.join("config/profiles.toml")
    }
}

/// `[fleet]`: the parking loop's numbers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Tunables {
    /// Seconds without a call before an active phone drops to normal CPU priority.
    pub(crate) idle_secs: u64,
    /// Seconds without a call before a phone is parked (QMP `stop`).
    pub(crate) park_secs: u64,
    /// Most phones running at once; past it the least recently used one is parked.
    pub(crate) max_awake: usize,
    /// Written to a parked phone's `memory.reclaim` (`memory_reclaim` on).
    pub(crate) reclaim_on_park: String,
    /// The cores an active phone gets: `auto` (`tools/pin-threads.sh --fast-cpus`) or a list.
    pub(crate) fast_cpus: String,
    /// Seconds a starting phone has before a failed health check counts.
    pub(crate) boot_secs: u64,
    /// Seconds of failed health checks before a running phone is restarted.
    pub(crate) unhealthy_secs: u64,
    /// Seconds a wake waits for `/v1/health`.
    pub(crate) wake_secs: u64,
    /// The first port a new phone's API (vphone-api-proxy) may take.
    pub(crate) api_port_base: u16,
}

impl Default for Tunables {
    fn default() -> Self {
        Self {
            idle_secs: 60,
            park_secs: 300,
            max_awake: 8,
            reclaim_on_park: "3G".to_owned(),
            fast_cpus: "auto".to_owned(),
            // A cold boot to unlock took 49-89 s on x86-64 (docs/DENSITY.md); the Spark is untested.
            boot_secs: 300,
            unhealthy_secs: 120,
            wake_secs: 30,
            // vphone-cli's documented --api-listen example port.
            api_port_base: 8765,
        }
    }
}

/// One switch per behaviour vphone-fleet adds, so an unstable one can be turned off alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Feature {
    Parking,
    MemoryReclaim,
    CpuPriority,
    ClockResync,
    AutoRestart,
    CorePinning,
    Batch,
    Events,
    QmpInput,
    Mcp,
}

impl Feature {
    pub(crate) const ALL: [Self; 10] = [
        Self::Parking,
        Self::MemoryReclaim,
        Self::CpuPriority,
        Self::ClockResync,
        Self::AutoRestart,
        Self::CorePinning,
        Self::Batch,
        Self::Events,
        Self::QmpInput,
        Self::Mcp,
    ];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Parking => "parking",
            Self::MemoryReclaim => "memory_reclaim",
            Self::CpuPriority => "cpu_priority",
            Self::ClockResync => "clock_resync",
            Self::AutoRestart => "auto_restart",
            Self::CorePinning => "core_pinning",
            Self::Batch => "batch",
            Self::Events => "events",
            Self::QmpInput => "qmp_input",
            Self::Mcp => "mcp",
        }
    }

    pub(crate) fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.name() == name)
    }

    /// On unless it needs host setup: memory reclaim needs zswap and swap
    /// (`tools/check-host.sh` says whether they are there).
    fn default_on(self) -> bool {
        self != Self::MemoryReclaim
    }

    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|f| *f == self)
            .expect("ALL lists every feature")
    }
}

/// A feature's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Switch {
    On,
    Off,
}

impl From<bool> for Switch {
    fn from(on: bool) -> Self {
        if on { Self::On } else { Self::Off }
    }
}

/// Where a feature's value came from; each overrides the one before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    Default,
    File,
    Env,
    /// `PUT /features/<name>`: live only, never written to the file.
    Api,
}

impl Source {
    fn name(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::File => "profiles.toml",
            Self::Env => "env",
            Self::Api => "api (not persisted)",
        }
    }
}

/// Every feature's value and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Features([(Switch, Source); Feature::ALL.len()]);

impl Default for Features {
    fn default() -> Self {
        Self(Feature::ALL.map(|f| (Switch::from(f.default_on()), Source::Default)))
    }
}

impl Features {
    pub(crate) fn on(&self, f: Feature) -> bool {
        self.0[f.index()].0 == Switch::On
    }

    pub(crate) fn set(&mut self, f: Feature, value: Switch, source: Source) {
        self.0[f.index()] = (value, source);
    }

    /// `VPHONE_FEATURE_<NAME>=0|1`, read through `var` (the environment, or a test's map).
    fn apply_env(&mut self, var: impl Fn(&str) -> Option<String>) -> Result<(), String> {
        for f in Feature::ALL {
            let key = format!("VPHONE_FEATURE_{}", f.name().to_uppercase());
            match var(&key).as_deref() {
                None => {}
                Some("1") => self.set(f, Switch::On, Source::Env),
                Some("0") => self.set(f, Switch::Off, Source::Env),
                Some(other) => return Err(format!("{key}={other}: use 0 or 1")),
            }
        }
        Ok(())
    }

    /// One line per feature: name, on/off, source.
    pub(crate) fn table(&self) -> String {
        let mut out = String::new();
        for f in Feature::ALL {
            let (value, src) = self.0[f.index()];
            let on = value == Switch::On;
            let _ = writeln!(
                out,
                "{:<15} {:<4} {}",
                f.name(),
                if on { "on" } else { "off" },
                src.name()
            );
        }
        out
    }

    /// `{"parking":{"on":true,"source":"default"},...}`.
    pub(crate) fn json(&self) -> String {
        let items: Vec<String> = Feature::ALL
            .iter()
            .map(|f| {
                let (value, src) = self.0[f.index()];
                let on = value == Switch::On;
                format!(
                    "\"{}\":{{\"on\":{on},\"source\":\"{}\"}}",
                    f.name(),
                    src.name()
                )
            })
            .collect();
        format!("{{{}}}", items.join(","))
    }
}

/// Everything `config/profiles.toml` and the environment decide for the fleet.
#[derive(Debug, Clone, Default)]
pub(crate) struct Config {
    pub(crate) rotate_ecid: bool,
    pub(crate) tunables: Tunables,
    pub(crate) features: Features,
    /// The `[profile.NAME]` names, for checking a phone's profile at create.
    pub(crate) profiles: Vec<String>,
}

impl Config {
    pub(crate) fn load(paths: &Paths) -> Result<Self, String> {
        let file = paths.profiles();
        let text =
            fs::read_to_string(&file).map_err(|e| format!("reading {}: {e}", file.display()))?;
        Self::parse(&text, |k| env::var(k).ok()).map_err(|e| format!("{}: {e}", file.display()))
    }

    fn parse(text: &str, var: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let table: Table = text.parse().map_err(|e| format!("{e}"))?;
        let mut c = Self::default();
        let section = |name: &str| -> Result<Table, String> {
            match table.get(name) {
                None => Ok(Table::new()),
                Some(Value::Table(t)) => Ok(t.clone()),
                Some(_) => Err(format!("[{name}] must be a table")),
            }
        };
        if let Some(v) = section("identity")?.get("rotate_ecid") {
            c.rotate_ecid = v
                .as_bool()
                .ok_or("[identity] rotate_ecid must be true or false")?;
        }
        let t = &mut c.tunables;
        for (k, v) in &section("fleet")? {
            let secs = || {
                v.as_integer()
                    .and_then(|n| u64::try_from(n).ok())
                    .ok_or(format!("[fleet] {k} must be a whole number of seconds"))
            };
            match k.as_str() {
                "idle_secs" => t.idle_secs = secs()?,
                "park_secs" => t.park_secs = secs()?,
                "boot_secs" => t.boot_secs = secs()?,
                "unhealthy_secs" => t.unhealthy_secs = secs()?,
                "wake_secs" => t.wake_secs = secs()?,
                "max_awake" => {
                    t.max_awake =
                        usize::try_from(secs()?).map_err(|e| format!("[fleet] {k}: {e}"))?;
                }
                "api_port_base" => {
                    t.api_port_base =
                        u16::try_from(secs()?).map_err(|e| format!("[fleet] {k}: {e}"))?;
                }
                "reclaim_on_park" | "fast_cpus" => {
                    let s = v.as_str().ok_or(format!("[fleet] {k} must be a string"))?;
                    if k == "fast_cpus" {
                        s.clone_into(&mut t.fast_cpus);
                    } else {
                        s.clone_into(&mut t.reclaim_on_park);
                    }
                }
                _ => return Err(format!("[fleet] {k}: unknown key")),
            }
        }
        for (k, v) in &section("features")? {
            let f = Feature::from_name(k).ok_or(format!("[features] {k}: unknown feature"))?;
            let on = v
                .as_bool()
                .ok_or(format!("[features] {k} must be true or false"))?;
            c.features.set(f, Switch::from(on), Source::File);
        }
        c.features.apply_env(var)?;
        c.profiles = section("profile")?.keys().cloned().collect();
        Ok(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_order_is_default_file_env() {
        let text = "[fleet]\nidle_secs = 5\nfast_cpus = \"0-3\"\n[features]\nparking = false\ncpu_priority = false\n[profile.lite]\nSMP = 4\n";
        let c = Config::parse(text, |k| {
            (k == "VPHONE_FEATURE_CPU_PRIORITY").then(|| "1".to_owned())
        })
        .unwrap();
        assert_eq!(c.tunables.idle_secs, 5);
        assert_eq!(c.tunables.park_secs, 300);
        assert_eq!(c.tunables.fast_cpus, "0-3");
        assert!(!c.features.on(Feature::Parking));
        assert!(c.features.on(Feature::CpuPriority));
        assert!(!c.features.on(Feature::MemoryReclaim), "off by default");
        assert!(c.features.table().contains("cpu_priority    on   env"));
        assert!(
            c.features
                .table()
                .contains("parking         off  profiles.toml")
        );
        assert_eq!(c.profiles, ["lite"]);
    }

    #[test]
    fn bad_values_are_refused() {
        for bad in [
            "[fleet]\nidle_secs = -1\n",
            "[fleet]\nnope = 1\n",
            "[features]\nparking = 1\n",
            "[features]\nwarp = true\n",
            "[identity]\nrotate_ecid = \"yes\"\n",
        ] {
            assert!(Config::parse(bad, |_| None).is_err(), "accepted {bad:?}");
        }
        let env_bad = Config::parse("", |k| {
            (k == "VPHONE_FEATURE_PARKING").then(|| "yes".to_owned())
        });
        assert!(env_bad.is_err());
    }

    #[test]
    fn the_checkouts_profiles_toml_parses() {
        let text = include_str!("../../../config/profiles.toml");
        let c = Config::parse(text, |_| None).unwrap();
        assert!(!c.rotate_ecid);
        assert_eq!(
            c.tunables,
            Tunables::default(),
            "profiles.toml and the defaults must agree"
        );
        for f in Feature::ALL {
            assert_eq!(
                c.features.on(f),
                Features::default().on(f),
                "profiles.toml and the default of {} must agree",
                f.name()
            );
        }
    }
}
