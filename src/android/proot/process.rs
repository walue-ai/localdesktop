use crate::android::utils::application_context::get_application_context;
use crate::core::{config, logging::PolarBearExpectation};
use std::io::BufRead;
use std::io::BufReader;
use std::io::Read;
use std::process::{Child, Command, Stdio};

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
        let distribution = context.local_config.distribution.name.clone();
        
        let fs_root = match distribution.as_str() {
            "void" => config::VOID_FS_ROOT,
            _ => config::ARCH_FS_ROOT,
        };

        #[cfg(not(test))]
        let proot_loader = context.native_library_dir.join("libproot_loader.so");
        #[cfg(test)]
        let proot_loader = "/data/local/tmp/libproot_loader.so";

        let mut process = Command::new(context.native_library_dir.join("libproot.so"));
        process
            .env("PROOT_LOADER", proot_loader)
            .env("PROOT_TMP_DIR", fs_root)
            .arg("-r")
            .arg(fs_root)
            .arg("-L")
            .arg("--link2symlink")
            .arg("--sysvipc")
            .arg("--kill-on-exit")
            .arg("--root-id")
            .arg("--bind=/dev")
            .arg("--bind=/proc")
            .arg("--bind=/sys")
            .arg(format!("--bind={}/tmp:/dev/shm", fs_root))
            .arg("--bind=/dev/urandom:/dev/random");
            
        #[cfg(not(target_os = "android"))]
        {
            process
                .arg("--bind=/proc/self/fd:/dev/fd")
                .arg("--bind=/proc/self/fd/0:/dev/stdin")
                .arg("--bind=/proc/self/fd/1:/dev/stdout")
                .arg("--bind=/proc/self/fd/2:/dev/stderr");
        }
        
        process
            .arg(format!("--bind={}/proc/.loadavg:/proc/loadavg", fs_root))
            .arg(format!("--bind={}/proc/.stat:/proc/stat", fs_root))
            .arg(format!("--bind={}/proc/.uptime:/proc/uptime", fs_root))
            .arg(format!("--bind={}/proc/.version:/proc/version", fs_root))
            .arg(format!("--bind={}/proc/.vmstat:/proc/vmstat", fs_root))
            .arg(format!("--bind={}/proc/.sysctl_entry_cap_last_cap:/proc/sys/kernel/cap_last_cap", fs_root))
            .arg(format!("--bind={}/proc/.sysctl_inotify_max_user_watches:/proc/sys/fs/inotify/max_user_watches", fs_root))
            .arg(format!("--bind={}/sys/.empty:/sys/fs/selinux", fs_root));

        let home = if self.user == "root" {
            "HOME=/root".to_string()
        } else {
            format!("HOME=/home/{}", self.user)
        };
        process.arg(home);

        process
            .arg("LANG=C.UTF-8")
            .arg("PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:/usr/local/games:/usr/games:/system/bin:/system/xbin")
            .arg("TMPDIR=/tmp")
            .arg("XDG_RUNTIME_DIR=/tmp")
            .arg("WAYLAND_DISPLAY=wayland-0")
            .arg("XDG_SESSION_TYPE=wayland")
            .arg(format!("USER={}", self.user))
            .arg(format!("LOGNAME={}", self.user));
            
        let command_parts: Vec<&str> = self.command.split_whitespace().collect();
        if !command_parts.is_empty() {
            let main_command = command_parts[0];
            process.arg(main_command);
            
            for arg in &command_parts[1..] {
                process.arg(arg);
            }
        } else {
            process.arg("echo").arg("No command specified");
        }
        
        let child = process
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
                if error_output.contains("fatal error: see `libproot.so --help`") {
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
