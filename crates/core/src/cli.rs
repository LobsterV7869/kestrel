use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invocation {
    /// Run the GUI, optionally launching a profile and joining a server.
    Gui {
        profile: Option<String>,
        join: Option<String>,
    },
    /// Headless benchmark harness.
    Bench { json: bool },
    Version,
    Help,
}

pub const HELP: &str = "\
Kestrel — a fast Minecraft: Java Edition launcher for Linux

USAGE:
    kestrel [OPTIONS]
    kestrel bench [--json]
    kestrel --profile <NAME> [--join <HOST:PORT>]

OPTIONS:
    -p, --profile <NAME>   Launch this profile immediately
        --join <HOST:PORT> Launch and join this multiplayer server
    -h, --help             Print this help
    -V, --version          Print version

COMMANDS:
    bench                  Measure cold-start and hot-path timings
";

pub fn parse_args<I>(args: I) -> Result<Invocation>
where
    I: IntoIterator<Item = String>,
{
    let mut profile = None;
    let mut join = None;
    let mut bench = false;
    let mut json = false;

    let mut iter = args.into_iter().peekable();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "bench" => bench = true,
            "--json" => json = true,
            "-h" | "--help" => return Ok(Invocation::Help),
            "-V" | "--version" => return Ok(Invocation::Version),
            "-p" | "--profile" => {
                let value = iter
                    .next()
                    .ok_or_else(|| Error::msg("--profile needs a value"))?;
                profile = Some(value);
            }
            "--join" => {
                let value = iter
                    .next()
                    .ok_or_else(|| Error::msg("--join needs host:port"))?;
                join = Some(value);
            }
            other if other.starts_with("--profile=") => {
                profile = Some(other["--profile=".len()..].to_string());
            }
            other if other.starts_with("--join=") => {
                join = Some(other["--join=".len()..].to_string());
            }
            other => {
                return Err(Error::msg(format!(
                    "unknown argument '{other}'. Try --help."
                )));
            }
        }
    }

    if bench {
        return Ok(Invocation::Bench { json });
    }
    Ok(Invocation::Gui { profile, join })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn gui_default() {
        assert_eq!(
            parse_args(args(&[])).unwrap(),
            Invocation::Gui {
                profile: None,
                join: None
            }
        );
    }

    #[test]
    fn profile_and_join() {
        assert_eq!(
            parse_args(args(&["--profile", "Optimized", "--join", "mc.example.net:25565"])).unwrap(),
            Invocation::Gui {
                profile: Some("Optimized".into()),
                join: Some("mc.example.net:25565".into())
            }
        );
    }

    #[test]
    fn equals_form() {
        assert_eq!(
            parse_args(args(&["--profile=Fabric", "--join=1.2.3.4"])).unwrap(),
            Invocation::Gui {
                profile: Some("Fabric".into()),
                join: Some("1.2.3.4".into())
            }
        );
    }

    #[test]
    fn bench_json() {
        assert_eq!(
            parse_args(args(&["bench", "--json"])).unwrap(),
            Invocation::Bench { json: true }
        );
    }

    #[test]
    fn help_wins() {
        assert_eq!(parse_args(args(&["--help"])).unwrap(), Invocation::Help);
    }

    #[test]
    fn unknown_arg_is_error() {
        assert!(parse_args(args(&["--nope"])).is_err());
    }
}
