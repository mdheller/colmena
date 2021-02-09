//! 
use std::convert::TryInto;
use std::collections::HashMap;
use std::fs;
use std::io::Write;

use async_trait::async_trait;
use tokio::process::Command;
use indicatif::ProgressBar;
use tempfile::NamedTempFile;

use super::{CopyDirection, CopyOptions, Host};
use crate::nix::{StorePath, Profile, DeploymentGoal, NixResult, NixCommand, Key, SYSTEM_PROFILE};
use crate::util::CommandExecution;

/// The local machine running Colmena.
///
/// It may not be capable of realizing some derivations
/// (e.g., building Linux derivations on macOS).
#[derive(Debug)]
pub struct Local {
    progress_bar: Option<ProgressBar>,
    logs: String,
}

impl Local {
    pub fn new() -> Self {
        Self {
            progress_bar: None,
            logs: String::new(),
        }
    }
}

#[async_trait]
impl Host for Local {
    async fn copy_closure(&mut self, _closure: &StorePath, _direction: CopyDirection, _options: CopyOptions) -> NixResult<()> {
        Ok(())
    }
    async fn realize_remote(&mut self, derivation: &StorePath) -> NixResult<Vec<StorePath>> {
        let mut command = Command::new("nix-store");
        command
            .arg("--no-gc-warning")
            .arg("--realise")
            .arg(derivation.as_path());

        let mut execution = CommandExecution::new("local", command);

        if let Some(bar) = self.progress_bar.as_ref() {
            execution.set_progress_bar(bar.clone());
        }

        let result = execution.run().await;

        let (stdout, stderr) = execution.get_logs();
        self.logs += stderr.unwrap();

        match result {
            Ok(()) => {
                stdout.unwrap().lines().map(|p| p.to_string().try_into()).collect()
            }
            Err(e) => Err(e),
        }
    }
    async fn upload_keys(&mut self, keys: &HashMap<String, Key>) -> NixResult<()> {
        for (name, key) in keys {
            self.upload_key(&name, &key).await?;
        }

        Ok(())
    }
    async fn activate(&mut self, profile: &Profile, goal: DeploymentGoal) -> NixResult<()> {
        if goal.should_switch_profile() {
            let path = profile.as_path().to_str().unwrap();
            Command::new("nix-env")
                .args(&["--profile", SYSTEM_PROFILE])
                .args(&["--set", path])
                .passthrough()
                .await?;
        }

        let activation_command = profile.activation_command(goal).unwrap();
        let mut command = Command::new(&activation_command[0]);
        command
            .args(&activation_command[1..]);

        let mut execution = CommandExecution::new("local", command);

        if let Some(bar) = self.progress_bar.as_ref() {
            execution.set_progress_bar(bar.clone());
        }

        let result = execution.run().await;

        // FIXME: Bad - Order of lines is messed up
        let (stdout, stderr) = execution.get_logs();
        self.logs += stdout.unwrap();
        self.logs += stderr.unwrap();

        result
    }
    fn set_progress_bar(&mut self, bar: ProgressBar) {
        self.progress_bar = Some(bar);
    }
    async fn dump_logs(&self) -> Option<&str> {
        Some(&self.logs)
    }
}

impl Local {
    /// "Uploads" a single key.
    async fn upload_key(&mut self, name: &str, key: &Key) -> NixResult<()> {
        if let Some(progress_bar) = self.progress_bar.as_ref() {
            progress_bar.set_message(&format!("Deploying key {}", name));
        }

        let dest_path = key.dest_dir.join(name);

        let mut temp = NamedTempFile::new()?;
        temp.write_all(key.text.as_bytes())?;

        let (_, temp_path) = temp.keep().map_err(|pe| pe.error)?;

        // Well, we need the userspace chmod program to parse the
        // permission, for NixOps compatibility
        {
            let mut command = Command::new("chmod");
            command
                .arg(&key.permissions)
                .arg(&temp_path);

            let mut execution = CommandExecution::new("local", command);
            execution.run().await?;
        }
        {
            let mut command = Command::new("chown");
            command
                .arg(&format!("{}:{}", key.user, key.group))
                .arg(&temp_path);

            let mut execution = CommandExecution::new("local", command);
            execution.run().await?;
        }

        let parent_dir = dest_path.parent().unwrap();
        fs::create_dir_all(parent_dir)?;
        fs::rename(temp_path, dest_path)?;

        Ok(())
    }
}