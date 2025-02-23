use super::utils::editor;
use anyhow::Result;
use directories::BaseDirs;
use log::{debug, LevelFilter};
use pretty_env_logger::formatted_timed_builder;
use serde::Deserialize;
use shellexpand;
use std::collections::BTreeMap;
use std::fs::write;
use std::path::PathBuf;
use std::process::Command;
use std::{env, fs};
use structopt::StructOpt;
use strum::{EnumIter, EnumString, EnumVariantNames, IntoEnumIterator};
use toml;

type Commands = BTreeMap<String, String>;

#[derive(EnumString, EnumVariantNames, Debug, Clone, PartialEq, Deserialize, EnumIter)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "snake_case")]
pub enum Step {
    System,
    PackageManagers,
    GitRepos,
    Vim,
    Emacs,
    Gem,
    Node,
    Sdkman,
    Remotes,
    Rustup,
    Cargo,
    Flutter,
    Go,
    Shell,
    Opam,
    Vcpkg,
    Pipx,
    Stack,
    Myrepos,
    Pearl,
    Jetpack,
    Atom,
    Firmware,
    Restarts,
}

#[derive(Deserialize, Default, Debug)]
#[serde(deny_unknown_fields)]
/// Configuration file
pub struct ConfigFile {
    pre_commands: Option<Commands>,
    commands: Option<Commands>,
    git_repos: Option<Vec<String>>,
    disable: Option<Vec<Step>>,
    remote_topgrades: Option<Vec<String>>,
    ssh_arguments: Option<String>,
    git_arguments: Option<String>,
    tmux_arguments: Option<String>,
    set_title: Option<bool>,
    assume_yes: Option<bool>,
    yay_arguments: Option<String>,
    no_retry: Option<bool>,
    run_in_tmux: Option<bool>,
    cleanup: Option<bool>,
    only: Option<Vec<Step>>,
}

impl ConfigFile {
    fn ensure(base_dirs: &BaseDirs) -> Result<PathBuf> {
        let config_path = base_dirs.config_dir().join("topgrade.toml");
        if !config_path.exists() {
            debug!("No configuration exists");
            write(&config_path, include_str!("../config.example.toml")).map_err(|e| {
                debug!(
                    "Unable to write the example configuration file to {}: {}. Using blank config.",
                    config_path.display(),
                    e
                );
                e
            })?;
        }

        Ok(config_path)
    }

    /// Read the configuration file.
    ///
    /// If the configuration file does not exist the function returns the default ConfigFile.
    fn read(base_dirs: &BaseDirs) -> Result<ConfigFile> {
        let config_path = Self::ensure(base_dirs)?;

        let contents = fs::read_to_string(&config_path).map_err(|e| {
            log::error!("Unable to read {}", config_path.display());
            e
        })?;

        let mut result: Self = toml::from_str(&contents).map_err(|e| {
            log::error!("Failed to deserialize {}", config_path.display());
            e
        })?;

        if let Some(ref mut paths) = &mut result.git_repos {
            for path in paths.iter_mut() {
                *path = shellexpand::tilde::<&str>(&path.as_ref()).into_owned();
            }
        }

        debug!("Loaded configuration: {:?}", result);

        Ok(result)
    }

    fn edit(base_dirs: &BaseDirs) -> Result<()> {
        let config_path = Self::ensure(base_dirs)?;
        let editor = editor();

        debug!("Editing {} with {}", config_path.display(), editor);
        Command::new(editor)
            .arg(config_path)
            .spawn()
            .and_then(|mut p| p.wait())?;
        Ok(())
    }
}

#[derive(StructOpt, Debug)]
#[structopt(name = "Topgrade", setting = structopt::clap::AppSettings::ColoredHelp)]
/// Command line arguments
pub struct CommandLineArgs {
    /// Edit the configuration file
    #[structopt(long = "edit-config")]
    edit_config: bool,

    /// Run inside tmux
    #[structopt(short = "t", long = "tmux")]
    run_in_tmux: bool,

    /// Cleanup temporary or old files
    #[structopt(short = "c", long = "cleanup")]
    cleanup: bool,

    /// Print what would be done
    #[structopt(short = "n", long = "dry-run")]
    dry_run: bool,

    /// Do not ask to retry failed steps
    #[structopt(long = "no-retry")]
    no_retry: bool,

    /// Do not perform upgrades for the given steps
    #[structopt(long = "disable", possible_values = &Step::variants())]
    disable: Vec<Step>,

    /// Perform only the specified steps (experimental)
    #[structopt(long = "only", possible_values = &Step::variants())]
    only: Vec<Step>,

    /// Output logs
    #[structopt(short = "v", long = "verbose")]
    verbose: bool,

    /// Prompt for a key before exiting
    #[structopt(short = "k", long = "keep")]
    keep_at_end: bool,

    /// Say yes to package manager's prompt (experimental)
    #[structopt(short = "y", long = "yes")]
    yes: bool,
}

impl CommandLineArgs {
    pub fn edit_config(&self) -> bool {
        self.edit_config
    }
}

/// Represents the application configuration
///
/// The struct holds the loaded configuration file, as well as the arguments parsed from the command line.
/// Its provided methods decide the appropriate options based on combining the configuraiton file and the
/// command line arguments.
pub struct Config {
    opt: CommandLineArgs,
    config_file: ConfigFile,
    allowed_steps: Vec<Step>,
}

impl Config {
    /// Load the configuration.
    ///
    /// The function parses the command line arguments and reading the configuration file.
    pub fn load(base_dirs: &BaseDirs, opt: CommandLineArgs) -> Result<Self> {
        let mut builder = formatted_timed_builder();

        if opt.verbose {
            builder.filter(Some("topgrade"), LevelFilter::Trace);
        }

        builder.init();

        let config_file = ConfigFile::read(base_dirs).unwrap_or_else(|e| {
            // Inform the user about errors when loading the configuration,
            // but fallback to the default config to at least attempt to do something
            log::error!("failed to load configuration: {}", e);
            ConfigFile::default()
        });

        let allowed_steps = Self::allowed_steps(&opt, &config_file);

        Ok(Self {
            opt,
            config_file,
            allowed_steps,
        })
    }

    /// Launch an editor to edit the configuration
    pub fn edit(base_dirs: &BaseDirs) -> Result<()> {
        ConfigFile::edit(base_dirs)
    }

    /// The list of commands to run before performing any step.
    pub fn pre_commands(&self) -> &Option<Commands> {
        &self.config_file.pre_commands
    }

    /// The list of custom steps.
    pub fn commands(&self) -> &Option<Commands> {
        &self.config_file.commands
    }

    /// The list of additional git repositories to pull.
    pub fn git_repos(&self) -> &Option<Vec<String>> {
        &self.config_file.git_repos
    }

    /// Tell whether the specified step should run.
    ///
    /// If the step appears either in the `--disable` command line argument
    /// or the `disable` option in the configuration, the function returns false.
    pub fn should_run(&self, step: Step) -> bool {
        self.allowed_steps.contains(&step)
    }

    fn allowed_steps(opt: &CommandLineArgs, config_file: &ConfigFile) -> Vec<Step> {
        let mut enabled_steps: Vec<Step> = if !opt.only.is_empty() {
            opt.only.clone()
        } else {
            config_file
                .only
                .as_ref()
                .map_or_else(|| Step::iter().collect(), |v| v.clone())
        };

        let disabled_steps: Vec<Step> = if !opt.disable.is_empty() {
            opt.disable.clone()
        } else {
            config_file.disable.as_ref().map_or_else(|| vec![], |v| v.clone())
        };

        enabled_steps.retain(|e| !disabled_steps.contains(e));
        enabled_steps
    }

    /// Tell whether we should run in tmux.
    pub fn run_in_tmux(&self) -> bool {
        self.opt.run_in_tmux || self.config_file.run_in_tmux.unwrap_or(false)
    }

    /// Tell whether we should perform cleanup steps.
    pub fn cleanup(&self) -> bool {
        self.opt.cleanup || self.config_file.cleanup.unwrap_or(false)
    }

    /// Tell whether we are dry-running.
    pub fn dry_run(&self) -> bool {
        self.opt.dry_run
    }

    /// Tell whether we should not attempt to retry anything.
    pub fn no_retry(&self) -> bool {
        self.opt.no_retry || self.config_file.no_retry.unwrap_or(false)
    }

    /// List of remote hosts to run Topgrade in
    pub fn remote_topgrades(&self) -> &Option<Vec<String>> {
        &self.config_file.remote_topgrades
    }

    /// Extra SSH arguments
    pub fn ssh_arguments(&self) -> &Option<String> {
        &self.config_file.ssh_arguments
    }

    /// Extra Git arguments
    pub fn git_arguments(&self) -> &Option<String> {
        &self.config_file.git_arguments
    }

    /// Extra Tmux arguments
    pub fn tmux_arguments(&self) -> &Option<String> {
        &self.config_file.tmux_arguments
    }

    /// Prompt for a key before exiting
    pub fn keep_at_end(&self) -> bool {
        self.opt.keep_at_end || env::var("TOPGRADE_KEEP_END").is_ok()
    }

    /// Whether to set the terminal title
    pub fn set_title(&self) -> bool {
        self.config_file.set_title.unwrap_or(true)
    }

    /// Whether to say yes to package managers
    #[allow(dead_code)]
    pub fn yes(&self) -> bool {
        self.config_file.assume_yes.unwrap_or(self.opt.yes)
    }

    /// Extra yay arguments
    #[cfg(target_os = "linux")]
    pub fn yay_arguments(&self) -> &str {
        match &self.config_file.yay_arguments {
            Some(args) => args.as_str(),
            None => "--devel",
        }
    }
}
