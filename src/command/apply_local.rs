use std::env;
use std::collections::HashMap;
use std::sync::Arc;

use clap::{Arg, App, SubCommand, ArgMatches};
use tokio::fs;
use tokio::process::Command;

use crate::nix::{Deployment, DeploymentGoal, Host};
use crate::nix::host;
use crate::util;

pub fn subcommand() -> App<'static, 'static> {
    SubCommand::with_name("apply-local")
        .about("Apply configurations on the local machine")
        .arg(Arg::with_name("goal")
            .help("Deployment goal")
            .long_help("Same as the targets for switch-to-configuration.\n\"push\" is noop in apply-local.")
            .default_value("switch")
            .index(1)
            .possible_values(&["push", "switch", "boot", "test", "dry-activate"]))
        .arg(Arg::with_name("sudo")
            .long("sudo")
            .help("Attempt to escalate privileges if not run as root")
            .takes_value(false))
        .arg(Arg::with_name("node")
            .long("node")
            .help("Override the node name to use")
            .takes_value(true))
        .arg(Arg::with_name("we-are-launched-by-sudo")
            .long("we-are-launched-by-sudo")
            .hidden(true)
            .takes_value(false))
}

pub async fn run(_global_args: &ArgMatches<'_>, local_args: &ArgMatches<'_>) {
    // Sanity check: Are we running NixOS?
    if let Ok(os_release) = fs::read_to_string("/etc/os-release").await {
        if !os_release.contains("ID=nixos\n") {
            log::error!("\"apply-local\" only works on NixOS machines.");
            quit::with_code(5);
        }
    } else {
        log::error!("Could not detect the OS version from /etc/os-release.");
        quit::with_code(5);
    }

    // Escalate privileges?
    {
        let euid: u32 = unsafe { libc::geteuid() };
        if euid != 0 {
            if local_args.is_present("we-are-launched-by-sudo") {
                log::error!("Failed to escalate privileges. We are still not root despite a successful sudo invocation.");
                quit::with_code(3);
            }

            if local_args.is_present("sudo") {
                escalate().await;
            } else {
                log::warn!("Colmena was not started by root. This is probably not going to work.");
                log::warn!("Hint: Add the --sudo flag.");
            }
        }
    }

    let hive = util::hive_from_args(local_args).unwrap();
    let hostname = if local_args.is_present("node") {
        local_args.value_of("node").unwrap().to_owned()
    } else {
        hostname::get().expect("Could not get hostname")
            .to_string_lossy().into_owned()
    };
    let goal = DeploymentGoal::from_str(local_args.value_of("goal").unwrap()).unwrap();

    log::info!("Enumerating nodes...");
    let all_nodes = hive.deployment_info().await.unwrap();

    let target: Box<dyn Host> = {
        if let Some(info) = all_nodes.get(&hostname) {
            if !info.allows_local_deployment() {
                log::error!("Local deployment is not enabled for host {}.", hostname);
                log::error!("Hint: Set deployment.allowLocalDeployment to true.");
                quit::with_code(2);
            }
            host::local()
        } else {
            log::error!("Host {} is not present in the Hive configuration.", hostname);
            quit::with_code(2);
        }
    };

    let mut targets = HashMap::new();
    targets.insert(hostname.clone(), target);

    let deployment = Arc::new(Deployment::new(hive, targets, goal));

    deployment.execute().await;
}

async fn escalate() -> ! {
    // Restart ourselves with sudo
    let argv: Vec<String> = env::args().collect();

    let exit = Command::new("sudo")
        .arg("--")
        .args(argv)
        .arg("--we-are-launched-by-sudo")
        .spawn()
        .expect("Failed to run sudo to escalate privileges")
        .wait()
        .await
        .expect("Failed to wait on child");

    // Exit with the same exit code
    quit::with_code(exit.code().unwrap());
}
