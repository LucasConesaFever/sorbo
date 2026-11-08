//! Command-line arguments of the `wm` binary.

use std::path::PathBuf;

/// What the command line asked for.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Args {
    /// `--log-file <path>`: log there instead of the terminal.
    pub log_file: Option<PathBuf>,
    /// `--check-accessibility`: exit 0 if this binary has the Accessibility permission,
    /// 1 if not, and do nothing else. A process caches its own answer, so a waiting WM asks
    /// a fresh copy of itself (RUN-9).
    pub check_accessibility: bool,
    /// `--check-config [path]`: report the config file's problems and exit (CFG-8).
    pub check_config: bool,
    /// The path given to `--check-config`; the usual config path when `None`.
    pub config_path: Option<PathBuf>,
}

/// Parses the arguments after the program name.
pub fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut parsed = Args::default();
    let mut args = args.into_iter().peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--log-file" => {
                let path = args.next().ok_or("--log-file needs a path")?;
                parsed.log_file = Some(PathBuf::from(path));
            }
            "--check-accessibility" => parsed.check_accessibility = true,
            "--check-config" => {
                parsed.check_config = true;
                // The path is optional: take the next argument unless it's another flag.
                if args.peek().is_some_and(|next| !next.starts_with("--")) {
                    parsed.config_path = args.next().map(PathBuf::from);
                }
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Args, String> {
        parse_args(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn run_5_no_arguments_logs_to_terminal() {
        assert_eq!(parse(&[]).unwrap().log_file, None);
    }

    #[test]
    fn run_5_log_file_argument() {
        assert_eq!(
            parse(&["--log-file", "/x/wm.log"]).unwrap().log_file,
            Some(PathBuf::from("/x/wm.log"))
        );
    }

    #[test]
    fn run_5_log_file_without_path_is_an_error() {
        assert!(parse(&["--log-file"]).is_err());
    }

    #[test]
    fn run_9_check_accessibility_flag() {
        assert!(
            parse(&["--check-accessibility"])
                .unwrap()
                .check_accessibility
        );
        assert!(!parse(&[]).unwrap().check_accessibility);
    }

    #[test]
    fn cfg_8_check_config_flag_with_and_without_path() {
        let without = parse(&["--check-config"]).unwrap();
        assert!(without.check_config);
        assert_eq!(without.config_path, None);
        let with = parse(&["--check-config", "/x/my.toml"]).unwrap();
        assert!(with.check_config);
        assert_eq!(with.config_path, Some(PathBuf::from("/x/my.toml")));
        assert!(!parse(&[]).unwrap().check_config);
    }

    #[test]
    fn run_5_unknown_argument_is_an_error() {
        assert!(parse(&["--foo"]).unwrap_err().contains("--foo"));
    }
}
