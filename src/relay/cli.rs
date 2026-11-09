//! `anyr relay` subcommand parsing. Pure: argv in, `Cmd` or a usage error out.

use crate::parse::ParsedArgs;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Cmd {
    /// Bare `anyr relay`: status plus the next command.
    Overview {
        json: bool,
    },
    Start {
        detach: bool,
    },
    Stop,
    Logs {
        follow: bool,
        lines: usize,
    },
    Status {
        json: bool,
    },
    Models {
        json: bool,
    },
    /// `relay models --devices`: per-model status across the account's devices.
    DeviceModels,
    Devices(super::devices::Devices),
    ModelSet {
        id: String,
        enable: bool,
    },
    /// `relay models auto [on|off]`: show or save the runtime model watch.
    ModelsAuto {
        set: Option<bool>,
        json: bool,
    },
    /// `relay models allow-all`: drop the local whitelist.
    ModelsAllowAll {
        json: bool,
    },
    Pool {
        set: Option<bool>,
        json: bool,
    },
    Pair,
    /// `relay rename <name>`: change this device's name here and in the cloud.
    Rename {
        name: String,
    },
    Unpair {
        yes: bool,
    },
    Doctor {
        json: bool,
    },
}

pub(super) const SUBCOMMANDS: &str =
    "start|stop|status|logs|models|devices|pool|pair|rename|unpair|doctor";

const DEFAULT_LOG_LINES: usize = 50;

fn usage(msg: &str) -> String {
    crate::cmd::dispatch::hint(&format!(
        "{msg}\nUsage: {{bin}} relay [{SUBCOMMANDS}] (see `{{bin}} relay --help`)"
    ))
}

/// Flags that only make sense on some subcommands. Rejecting them keeps a
/// typo like `relay stop --detach` from silently doing something else.
fn only_on(parsed: &ParsedArgs, flag: &str, allowed: &[&str], sub: &str) -> Result<(), String> {
    if parsed.flags.contains_key(flag) && !allowed.contains(&sub) {
        return Err(usage(&format!("--{flag} does not apply to `relay {sub}`.")));
    }
    Ok(())
}

pub(super) fn parse_cmd(parsed: &ParsedArgs) -> Result<Cmd, String> {
    let words: Vec<&str> = parsed.passthrough.iter().map(String::as_str).collect();
    let sub = words.first().copied().unwrap_or("");
    let json = parsed.flag_true("json");
    let label = if sub.is_empty() { "relay" } else { sub };
    only_on(
        parsed,
        "json",
        &["", "status", "models", "pool", "doctor", "devices"],
        sub,
    )
    .map_err(|_| usage(&format!("--json does not apply to `{label}`.")))?;
    only_on(parsed, "detach", &["start"], sub)?;
    only_on(parsed, "follow", &["logs"], sub)?;
    only_on(parsed, "lines", &["logs"], sub)?;
    only_on(parsed, "yes", &["unpair", "devices"], sub)?;
    only_on(parsed, "devices", &["models"], sub)?;
    let extra = |n: usize| -> Result<(), String> {
        match words.get(n) {
            Some(w) => Err(usage(&format!(
                "Unexpected argument \"{w}\" for `relay {sub}`."
            ))),
            None => Ok(()),
        }
    };
    let cmd = match sub {
        "" => Cmd::Overview { json },
        "start" => {
            extra(1)?;
            Cmd::Start {
                detach: parsed.flag_true("detach"),
            }
        }
        "stop" => {
            extra(1)?;
            Cmd::Stop
        }
        "logs" => {
            extra(1)?;
            let lines = match crate::parse::get_string_flag(&parsed.flags, "lines") {
                None => DEFAULT_LOG_LINES,
                Some(raw) => raw
                    .parse::<usize>()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| {
                        usage(&format!("--lines takes a positive number, got \"{raw}\"."))
                    })?,
            };
            Cmd::Logs {
                follow: parsed.flag_true("follow"),
                lines,
            }
        }
        "status" => {
            extra(1)?;
            Cmd::Status { json }
        }
        "models" => match words.get(1).copied() {
            None | Some("list") | Some("ls") => {
                extra(2)?;
                if parsed.flag_true("devices") {
                    Cmd::DeviceModels
                } else {
                    Cmd::Models { json }
                }
            }
            Some(verb @ ("enable" | "disable")) => {
                let Some(id) = words.get(2) else {
                    return Err(usage(&format!(
                        "`relay models {verb}` needs a model id, e.g. `{{bin}} relay models {verb} apple/foundation-model`."
                    )));
                };
                extra(3)?;
                Cmd::ModelSet {
                    id: id.to_string(),
                    enable: verb == "enable",
                }
            }
            Some("auto") => {
                let set = match words.get(2).copied() {
                    None => None,
                    Some("on") => Some(true),
                    Some("off") => Some(false),
                    Some(other) => {
                        return Err(usage(&format!(
                            "`relay models auto` takes on or off, got \"{other}\"."
                        )))
                    }
                };
                extra(3)?;
                Cmd::ModelsAuto { set, json }
            }
            Some("allow-all") => {
                extra(2)?;
                Cmd::ModelsAllowAll { json }
            }
            Some(other) => {
                return Err(usage(&format!(
                    "Unknown `relay models` action \"{other}\" (use enable, disable, auto or allow-all)."
                )))
            }
        },
        "devices" => Cmd::Devices(
            super::devices::parse(&words[1..], parsed.skip_confirm()).map_err(|e| usage(&e))?,
        ),
        "pool" => {
            let set = match words.get(1).copied() {
                None => None,
                Some("on") => Some(true),
                Some("off") => Some(false),
                Some(other) => {
                    return Err(usage(&format!(
                        "`relay pool` takes on or off, got \"{other}\"."
                    )))
                }
            };
            extra(2)?;
            Cmd::Pool { set, json }
        }
        "pair" => {
            extra(1)?;
            Cmd::Pair
        }
        "rename" => {
            let Some(name) = words.get(1).filter(|w| !w.trim().is_empty()) else {
                return Err(usage(
                    "`relay rename` needs a name, e.g. `{bin} relay rename \"Studio\"`.",
                ));
            };
            extra(2)?;
            Cmd::Rename {
                name: name.to_string(),
            }
        }
        "unpair" => {
            extra(1)?;
            Cmd::Unpair {
                yes: parsed.skip_confirm(),
            }
        }
        "doctor" => {
            extra(1)?;
            Cmd::Doctor { json }
        }
        other => return Err(usage(&format!("Unknown relay subcommand: {other}"))),
    };
    Ok(cmd)
}

/// `--pool` / `--no-pool` as an explicit override. None = not passed.
pub(super) fn pool_flag(parsed: &ParsedArgs) -> Result<Option<bool>, String> {
    match (parsed.flag_true("pool"), parsed.flag_true("no-pool")) {
        (true, true) => Err(usage("Pass --pool or --no-pool, not both.")),
        (true, false) => Ok(Some(true)),
        (false, true) => Ok(Some(false)),
        (false, false) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_needs_exactly_one_name() {
        let ok = parse_cmd(&p(&["rename", "Studio"])).unwrap();
        assert_eq!(
            ok,
            Cmd::Rename {
                name: "Studio".into()
            }
        );
        assert!(parse_cmd(&p(&["rename"])).is_err());
        assert!(parse_cmd(&p(&["rename", "a", "b"])).is_err());
    }

    fn p(argv: &[&str]) -> ParsedArgs {
        let mut full = vec!["relay"];
        full.extend_from_slice(argv);
        crate::parse::parse_cli_args(full).unwrap()
    }

    fn cmd(argv: &[&str]) -> Cmd {
        parse_cmd(&p(argv)).unwrap()
    }

    fn err(argv: &[&str]) -> String {
        parse_cmd(&p(argv)).unwrap_err()
    }

    #[test]
    fn every_subcommand_parses() {
        assert_eq!(cmd(&[]), Cmd::Overview { json: false });
        assert_eq!(cmd(&["--json"]), Cmd::Overview { json: true });
        assert_eq!(cmd(&["start"]), Cmd::Start { detach: false });
        assert_eq!(cmd(&["start", "--detach"]), Cmd::Start { detach: true });
        assert_eq!(cmd(&["stop"]), Cmd::Stop);
        assert_eq!(
            cmd(&["logs", "--follow", "--lines", "5"]),
            Cmd::Logs {
                follow: true,
                lines: 5
            }
        );
        assert_eq!(
            cmd(&["logs"]),
            Cmd::Logs {
                follow: false,
                lines: 50
            }
        );
        assert_eq!(cmd(&["status", "--json"]), Cmd::Status { json: true });
        assert_eq!(cmd(&["models"]), Cmd::Models { json: false });
        assert_eq!(
            cmd(&["models", "list", "--json"]),
            Cmd::Models { json: true }
        );
        assert_eq!(
            cmd(&["models", "disable", "apple/foundation-model"]),
            Cmd::ModelSet {
                id: "apple/foundation-model".into(),
                enable: false
            }
        );
        assert_eq!(
            cmd(&["models", "auto"]),
            Cmd::ModelsAuto {
                set: None,
                json: false
            }
        );
        assert_eq!(
            cmd(&["models", "auto", "on"]),
            Cmd::ModelsAuto {
                set: Some(true),
                json: false
            }
        );
        assert_eq!(
            cmd(&["models", "auto", "off", "--json"]),
            Cmd::ModelsAuto {
                set: Some(false),
                json: true
            }
        );
        assert_eq!(
            cmd(&["models", "allow-all"]),
            Cmd::ModelsAllowAll { json: false }
        );
        assert!(parse_cmd(&p(&["models", "auto", "maybe"])).is_err());
        assert_eq!(
            cmd(&["pool"]),
            Cmd::Pool {
                set: None,
                json: false
            }
        );
        assert_eq!(
            cmd(&["pool", "off"]),
            Cmd::Pool {
                set: Some(false),
                json: false
            }
        );
        assert_eq!(
            cmd(&["pool", "on", "--json"]),
            Cmd::Pool {
                set: Some(true),
                json: true
            }
        );
        assert_eq!(cmd(&["pair"]), Cmd::Pair);
        assert_eq!(cmd(&["unpair", "--yes"]), Cmd::Unpair { yes: true });
        assert_eq!(cmd(&["unpair"]), Cmd::Unpair { yes: false });
        assert_eq!(cmd(&["doctor", "--json"]), Cmd::Doctor { json: true });
        assert_eq!(cmd(&["models", "--devices"]), Cmd::DeviceModels);
        assert_eq!(
            cmd(&["devices", "rm", "d1", "--yes"]),
            Cmd::Devices(super::super::devices::Devices::Rm {
                id: "d1".into(),
                yes: true
            })
        );
        assert_eq!(
            cmd(&["devices", "rm", "d1"]),
            Cmd::Devices(super::super::devices::Devices::Rm {
                id: "d1".into(),
                yes: false
            })
        );
    }

    #[test]
    fn usage_errors_are_marked_for_exit_2_and_name_the_problem() {
        for (argv, needle) in [
            (&["wat"][..], "Unknown relay subcommand: wat"),
            (&["pool", "maybe"], "on or off"),
            (&["models", "enable"], "needs a model id"),
            (
                &["models", "toggle", "x"],
                "enable, disable, auto or allow-all",
            ),
            (&["stop", "--detach"], "--detach does not apply"),
            (&["start", "--json"], "--json does not apply"),
            (&["logs", "--lines", "0"], "positive number"),
            (&["devices", "rename", "d1"], "needs a new name"),
            (&["devices", "pool", "d1", "x"], "on or off"),
            (&["devices", "--detach"], "--detach does not apply"),
            (&["status", "--devices"], "--devices does not apply"),
            (&["status", "extra"], "Unexpected argument \"extra\""),
        ] {
            let e = err(argv);
            assert!(
                e.starts_with(crate::cmd::dispatch::USAGE),
                "{argv:?} must be a usage error"
            );
            assert!(e.contains(needle), "{argv:?}: {e}");
            assert!(e.contains("Usage:"), "{argv:?}: {e}");
        }
    }

    #[test]
    fn pool_flags_are_an_override_and_conflict_when_both_given() {
        assert_eq!(pool_flag(&p(&["start"])).unwrap(), None);
        assert_eq!(pool_flag(&p(&["start", "--pool"])).unwrap(), Some(true));
        assert_eq!(pool_flag(&p(&["start", "--no-pool"])).unwrap(), Some(false));
        assert!(pool_flag(&p(&["start", "--pool", "--no-pool"])).is_err());
    }
}
