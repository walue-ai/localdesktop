use crate::android::utils::application_context::get_application_context;
use crate::core::{config, logging::PolarBearExpectation};
use std::io::BufRead;
use std::io::BufReader;
use std::io::Read;
use std::process::{Child, Command, Stdio};
use log;

pub type Log = Box<dyn Fn(String)>;

pub struct ArchProcess {
    pub command: String,
    pub user: String,
    pub process: Option<Child>,
    pub panic_on_error: bool,
}

impl ArchProcess {
    pub fn spawn(mut self) -> Self {
        // Run the command inside Proot
        let context = get_application_context();

        #[cfg(not(test))]
        let proot_loader = context.native_library_dir.join("libproot_loader.so");
        #[cfg(test)]
        let proot_loader = "/data/local/tmp/libproot_loader.so";

        let mut process = Command::new(context.native_library_dir.join("libproot-userland.so"));
        process
            .env("PROOT_LOADER", proot_loader)
            .env("PROOT_TMP_DIR", "/data/data/app.polarbear/cache")
            .env("PROOT_NO_SECCOMP", "1")
            .env("PROOT_VERBOSE", "9")
            .env("PROOT_IGNORE_MISSING_BINDINGS", "1")
            .env("PROOT_F2FS_WORKAROUND", "1")
            .arg("-r")
            .arg(config::ARCH_FS_ROOT)
            .arg("--bind=/dev/urandom:/dev/random")
            .arg("/usr/bin/bash")
            .arg("-l");

        log::info!("Launching PRoot with envs: {:?}", process.get_envs().collect::<Vec<_>>());

        if self.user != "root" {
            process.env("USER", &self.user);
            process.env("LOGNAME", &self.user);
            process.env("HOME", format!("/home/{}", self.user));
        } else {
            process.env("USER", "root");
            process.env("LOGNAME", "root");
            process.env("HOME", "/root");
        }
        
        process.env("LANG", "C.UTF-8");
        process.env("PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:/usr/local/games:/usr/games:/system/bin:/system/xbin");
        process.env("TMPDIR", "/tmp");
        let child = process
            .arg("-c")
            .arg(&self.command)
            .stdout(Stdio::piped())
            .stderr(if self.panic_on_error {
                Stdio::piped()
            } else {
                Stdio::inherit()
            })
            .spawn()
            .pb_expect("Failed to run command");

        self.process.replace(child);
        self
    }

    pub fn exec(command: &str) -> Self {
        ArchProcess {
            command: command.to_string(),
            user: "root".to_string(),
            process: None,
            panic_on_error: false,
        }
        .spawn()
    }

    pub fn exec_as(command: &str, user: &str) -> Self {
        ArchProcess {
            command: command.to_string(),
            user: user.to_string(),
            process: None,
            panic_on_error: false,
        }
        .spawn()
    }

    pub fn with_log(self, mut log: impl FnMut(String)) {
        if let Some(child) = self.process {
            let reader = BufReader::new(child.stdout.unwrap());
            for line in reader.lines() {
                let line = line.unwrap();
                log(line);
            }
        }
    }

    pub fn wait_with_output(self) -> std::io::Result<std::process::Output> {
        if let Some(child) = self.process {
            child.wait_with_output()
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "Process not spawned",
            ))
        }
    }

    pub fn wait(self) -> std::io::Result<std::process::ExitStatus> {
        if let Some(mut child) = self.process {
            child.wait()
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "Process not spawned",
            ))
        }
    }

    pub fn exec_with_panic_on_error(command: &str) {
        if let Some(child) = (ArchProcess {
            command: command.to_string(),
            user: "root".to_string(),
            process: None,
            panic_on_error: true,
        }
        .spawn()
        .process)
        {
            // What is the best way to get full stderr as a string?
            if let Some(stderr) = child.stderr {
                let mut error_output = String::new();
                let mut reader = BufReader::new(stderr);
                reader.read_to_string(&mut error_output).unwrap();
                if error_output.contains("fatal error: see `libproot-userland.so --help`") {
                    panic!("PRoot error: {}", error_output);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[test]
    fn should_echoable() {
        let process = ArchProcess::exec("echo hello");
        let output = process.wait_with_output().expect("Failed to read output");
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "hello");
    }

    #[test]
    fn should_output_uname() {
        let process = ArchProcess::exec("uname -a");
        let output = process.wait_with_output().expect("Failed to read output");
        log::info!("Output: {}", String::from_utf8_lossy(&output.stdout));
        assert!(String::from_utf8_lossy(&output.stdout)
            .to_lowercase()
            .contains("arch"));
    }

    #[test]
    fn should_run_with_log_successfully() {
        let mut logs = VecDeque::new();
        ArchProcess {
            command: "echo hello".to_string(),
            user: "root".to_string(),
            process: None,
            panic_on_error: true,
        }
        .spawn()
        .with_log(|log| {
            logs.push_back(log.to_string());
        });
        assert!(logs.iter().any(|log| log.contains("hello")));
    }

    #[test]
    fn should_exit_with_success_code() {
        let process = ArchProcess::exec("pacman -Ss chrome");
        let status = process.wait().expect("Failed to wait for process");
        assert_eq!(status.success(), true);
    }

    #[test]
    fn should_exit_with_fail_code() {
        let process = ArchProcess::exec("pacman -Qg plasmma");
        let status = process.wait().expect("Failed to wait for process");
        assert_ne!(status.success(), true);
    }
}
