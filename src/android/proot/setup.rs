use super::process::ArchProcess;
use crate::{
    android::{
        app::build::PolarBearBackend,
        backend::{
            wayland::{Compositor, WaylandBackend},
            webview::WebviewBackend,
        },
        utils::application_context::get_application_context,
    },
    core::{
        config::{ARCH_FS_ARCHIVE, ARCH_FS_ROOT, VOID_FS_ARCHIVE, VOID_FS_ROOT, ALPINE_FS_ARCHIVE, ALPINE_FS_ROOT},
        logging::PolarBearExpectation,
    },
};
use pathdiff::diff_paths;
use smithay::utils::Clock;
use std::{
    fs::{self, File},
    io::{Read, Write},
    os::unix::fs::{symlink, PermissionsExt},
    path::Path,
    sync::{
        mpsc::{self, Sender},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
};
use tar::Archive;
use winit::platform::android::activity::AndroidApp;
use xz2::read::XzDecoder;

#[derive(Debug)]
pub enum SetupMessage {
    Progress(String),
    Error(String),
}

pub struct SetupOptions {
    pub android_app: AndroidApp,
    pub mpsc_sender: Sender<SetupMessage>,
}

/// Setup is a process that should be done **only once** when the user installed the app.
/// The setup process consists of several stages.
/// Each stage is a function that takes the `SetupOptions` and returns a `StageOutput`.
type SetupStage = Box<dyn Fn(&SetupOptions) -> StageOutput + Send>;

/// Each stage should indicate whether the associated task is done previously or not.
/// Thus, it should return a finished status if the task is done, so that the setup process can move on to the next stage.
/// Otherwise, it should return a `JoinHandle`, so that the setup process can wait for the task to finish, but not block the main thread so that the setup progress can be reported to the user.
type StageOutput = Option<JoinHandle<()>>;

fn setup_linux_fs(options: &SetupOptions) -> StageOutput {
    let context = get_application_context();
    let distribution = context.local_config.distribution.name.clone();
    
    let (fs_root, archive_url, temp_filename, extracted_dirname) = match distribution.as_str() {
        "void" => (
            Path::new(VOID_FS_ROOT),
            VOID_FS_ARCHIVE,
            "voidlinux-fs.tar.xz",
            "void-aarch64"
        ),
        "alpine" => (
            Path::new(ALPINE_FS_ROOT),
            ALPINE_FS_ARCHIVE,
            "alpine-fs.tar.xz",
            "alpine-aarch64-pd-v4.25.0"
        ),
        _ => (
            Path::new(ARCH_FS_ROOT),
            ARCH_FS_ARCHIVE,
            "archlinux-fs.tar.xz",
            "archlinux-aarch64"
        ),
    };
    
    let temp_file = context.data_dir.join(temp_filename);
    let mpsc_sender = options.mpsc_sender.clone();

    // Only run if the fs_root is missing, empty, or incomplete (missing essential tools)
    // TODO: Setup integration test to make sure on clean install, the fs_root is either non existent or empty
    let is_empty = fs_root.read_dir().map_or(true, |mut d| d.next().is_none());
    
    let is_complete = if !is_empty {
        match distribution.as_str() {
            "alpine" => fs_root.join("sbin/apk").exists(),
            "void" => fs_root.join("usr/bin/xbps-query").exists() && fs_root.join("usr/bin/xbps-install").exists(),
            _ => fs_root.join("usr/bin/pacman").exists(),
        }
    } else {
        false
    };
    
    let need_setup = is_empty || !is_complete;
    
    log::info!("=== EXTRACTION DECISION LOGIC ===");
    log::info!("Distribution: {}", distribution);
    log::info!("Rootfs path: {:?}", fs_root);
    log::info!("Directory is empty: {}", is_empty);
    log::info!("Directory is complete: {}", is_complete);
    log::info!("Need setup/extraction: {}", need_setup);
    
    if need_setup {
        return Some(thread::spawn(move || {
            // Download if the archive doesn't exist
            loop {
                if !temp_file.exists() {
                    mpsc_sender
                        .send(SetupMessage::Progress(
                            format!("Downloading {} Linux FS...", distribution).to_string(),
                        ))
                        .pb_expect("Failed to send log message");

                    let response = reqwest::blocking::get(archive_url)
                        .pb_expect(&format!("Failed to download {} Linux FS", distribution));

                    let total_size = response.content_length().unwrap_or(0);
                    let mut file = File::create(&temp_file)
                        .pb_expect(&format!("Failed to create temp file for {} Linux FS", distribution));

                    let mut downloaded = 0u64;
                    let mut buffer = [0u8; 8192];
                    let mut reader = response;
                    let mut last_percent = 0;

                    loop {
                        let n = reader
                            .read(&mut buffer)
                            .pb_expect("Failed to read from response");
                        if n == 0 {
                            break;
                        }
                        file.write_all(&buffer[..n])
                            .pb_expect("Failed to write to file");
                        downloaded += n as u64;
                        if total_size > 0 {
                            let percent = (downloaded * 100 / total_size).min(100) as u8;
                            if percent != last_percent {
                                let downloaded_mb = downloaded as f64 / 1024.0 / 1024.0;
                                let total_mb = total_size as f64 / 1024.0 / 1024.0;
                                mpsc_sender
                                    .send(SetupMessage::Progress(format!(
                                        "Downloading {} Linux FS... {}% ({:.2} MB / {:.2} MB)",
                                        distribution, percent, downloaded_mb, total_mb
                                    )))
                                    .unwrap_or(());
                                last_percent = percent;
                            }
                        }
                    }
                }

                mpsc_sender
                    .send(SetupMessage::Progress(
                        format!("Extracting {} Linux FS...", distribution).to_string(),
                    ))
                    .pb_expect("Failed to send log message");

                log::info!("=== STARTING {} LINUX ROOTFS EXTRACTION ===", distribution.to_uppercase());
                log::info!("Source file: {:?}", temp_file);
                log::info!("Target directory: {:?}", fs_root);
                log::info!("Expected extracted dirname: {}", extracted_dirname);

                // Ensure the final destination is clean
                let _ = fs::remove_dir_all(fs_root);
                log::info!("Cleaned target directory: {:?}", fs_root);

                // Extract tar file directly to the final destination
                let tar_file = File::open(&temp_file)
                    .pb_expect(&format!("Failed to open downloaded {} Linux FS file", distribution));
                
                log::info!("Opened tar file successfully, file size: {} bytes", 
                    tar_file.metadata().map(|m| m.len()).unwrap_or(0));
                
                let mut archive: Archive<Box<dyn Read>> = {
                    let xz = XzDecoder::new(tar_file);
                    Archive::new(Box::new(xz))
                };

                archive.set_overwrite(true);
                archive.set_preserve_permissions(false);
                archive.set_preserve_ownerships(false);
                
                log::info!("Archive configured, starting extraction...");
                let mut extracted_files = 0;
                let mut extracted_dirs = 0;
                let mut skipped_links = 0;
                
                let extract_result = (|| -> Result<(), Box<dyn std::error::Error>> {
                    for entry in archive.entries()? {
                        let mut entry = entry?;
                        let path = entry.path()?.to_path_buf();
                        let header = entry.header().clone();
                        
                        if header.entry_type() == tar::EntryType::Link {
                            log::debug!("Skipping hard link: {:?}", path);
                            skipped_links += 1;
                            continue;
                        }
                        
                        let path_str = path.to_string_lossy();
                        if path_str.contains("apk") || path_str.contains("bin/") || path_str.contains("sbin/") {
                            log::info!("Extracting important file: {:?} (type: {:?})", path, header.entry_type());
                        }
                        
                        entry.unpack_in(fs_root.parent().unwrap())?;
                        
                        match header.entry_type() {
                            tar::EntryType::Directory => extracted_dirs += 1,
                            tar::EntryType::Regular => extracted_files += 1,
                            _ => {}
                        }
                        
                        if (extracted_files + extracted_dirs) % 1000 == 0 {
                            log::info!("Extraction progress: {} files, {} dirs extracted", extracted_files, extracted_dirs);
                        }
                    }
                    Ok(())
                })();
                
                log::info!("Extraction completed - Files: {}, Dirs: {}, Skipped links: {}", 
                    extracted_files, extracted_dirs, skipped_links);
                
                // Try to extract, if it fails, remove temp file and restart download
                if let Err(e) = extract_result {
                    log::error!("❌ EXTRACTION FAILED: {}", e);
                    log::error!("Cleaning up failed extraction and retrying download...");
                    
                    // Clean up the failed extraction
                    let _ = fs::remove_dir_all(fs_root);
                    let _ = fs::remove_file(&temp_file);

                    mpsc_sender
                        .send(SetupMessage::Error(format!(
                            "Failed to extract {} Linux FS: {}. Restarting download...",
                            distribution, e
                        )))
                        .unwrap_or(());

                    // Continue the outer loop to retry the download
                    continue;
                }

                // If we get here, extraction was successful
                log::info!("✅ EXTRACTION SUCCESSFUL!");
                
                log::info!("=== POST-EXTRACTION DIRECTORY VERIFICATION ===");
                if fs_root.exists() {
                    log::info!("✅ Target directory exists: {:?}", fs_root);
                    
                    if let Ok(entries) = std::fs::read_dir(fs_root) {
                        let mut dirs = Vec::new();
                        let mut files = Vec::new();
                        for entry in entries.flatten() {
                            let name = entry.file_name().to_string_lossy().to_string();
                            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                                dirs.push(name);
                            } else {
                                files.push(name);
                            }
                        }
                        log::info!("Top-level directories: {:?}", dirs);
                        log::info!("Top-level files: {:?}", files);
                    }
                } else {
                    log::error!("❌ Target directory does not exist after extraction: {:?}", fs_root);
                }
                
                // SOLUTION A & B: Verify package manager tools after successful extraction
                log::info!("=== STARTING PACKAGE MANAGER VERIFICATION ===");
                verify_package_manager_tools(fs_root, &distribution, &mpsc_sender);
                log::info!("=== PACKAGE MANAGER VERIFICATION COMPLETE ===");
                
                break;
            }

            // Clean up the temporary file
            fs::remove_file(&temp_file).pb_expect("Failed to remove temporary file");
        }));
    }
    None
}

fn simulate_linux_sysdata_stage(options: &SetupOptions) -> StageOutput {
    let context = get_application_context();
    let distribution = context.local_config.distribution.name.clone();
    
    let fs_root = match distribution.as_str() {
        "void" => Path::new(VOID_FS_ROOT),
        "alpine" => Path::new(ALPINE_FS_ROOT),
        _ => Path::new(ARCH_FS_ROOT),
    };
    
    let mpsc_sender = options.mpsc_sender.clone();

    if !fs_root.join("proc/.version").exists() {
        return Some(thread::spawn(move || {
            mpsc_sender
                .send(SetupMessage::Progress(
                    "Simulating Linux system data...".to_string(),
                ))
                .pb_expect(&format!("Failed to send log message"));

            // Create necessary directories - don't fail if they already exist
            let _ = fs::create_dir_all(fs_root.join("proc"));
            let _ = fs::create_dir_all(fs_root.join("sys"));
            let _ = fs::create_dir_all(fs_root.join("sys/.empty"));
            let _ = fs::create_dir_all(fs_root.join("tmp"));
            let _ = fs::create_dir_all(fs_root.join("usr/bin"));
            let _ = fs::create_dir_all(fs_root.join("dev"));

            // Set permissions - only try to set permissions if we're on Unix and have the capability
            #[cfg(unix)]
            {
                // Try to set permissions, but don't fail if we can't
                let _ =
                    fs::set_permissions(fs_root.join("proc"), fs::Permissions::from_mode(0o700));
                let _ = fs::set_permissions(fs_root.join("sys"), fs::Permissions::from_mode(0o700));
                let _ = fs::set_permissions(
                    fs_root.join("sys/.empty"),
                    fs::Permissions::from_mode(0o700),
                );
                let _ = fs::set_permissions(fs_root.join("tmp"), fs::Permissions::from_mode(0o1777)); // Sticky bit for tmp
                let _ = fs::set_permissions(fs_root.join("usr/bin"), fs::Permissions::from_mode(0o755));
                let _ = fs::set_permissions(fs_root.join("dev"), fs::Permissions::from_mode(0o755));
            }

            // Create fake proc files
            let proc_files = [
                    ("proc/.loadavg", "0.12 0.07 0.02 2/165 765\n"),
                    ("proc/.stat", "cpu  1957 0 2877 93280 262 342 254 87 0 0\ncpu0 31 0 226 12027 82 10 4 9 0 0\n"),
                    ("proc/.uptime", "124.08 932.80\n"),
                    ("proc/.version", "Linux version 6.2.1 (proot@termux) (gcc (GCC) 12.2.1 20230201, GNU ld (GNU Binutils) 2.40) #1 SMP PREEMPT_DYNAMIC Wed, 01 Mar 2023 00:00:00 +0000\n"),
                    ("proc/.vmstat", "nr_free_pages 1743136\nnr_zone_inactive_anon 179281\nnr_zone_active_anon 7183\n"),
                    ("proc/.sysctl_entry_cap_last_cap", "40\n"),
                    ("proc/.sysctl_inotify_max_user_watches", "4096\n"),
                ];

            for (path, content) in proc_files {
                let _ = fs::write(fs_root.join(path), content)
                    .pb_expect(&format!("Permission denied while writing to {}", path));
            }

            // Create /usr/bin/env if it doesn't exist - essential for many scripts
            let env_path = fs_root.join("usr/bin/env");
            if !env_path.exists() {
                // Create a simple shell script that acts as env
                let env_script = r#"#!/bin/sh
# Simple env replacement for Android/PRoot environment
if [ $# -eq 0 ]; then
    # No arguments - print environment
    printenv
else
    # Execute command with environment
    exec "$@"
fi
"#;
                let _ = fs::write(&env_path, env_script)
                    .pb_expect("Failed to create /usr/bin/env");
                
                #[cfg(unix)]
                {
                    let _ = fs::set_permissions(&env_path, fs::Permissions::from_mode(0o755));
                }
            }


            // Create /bin directory and /bin/sh if they don't exist - essential for PRoot shell execution
            fs::create_dir_all(fs_root.join("bin"))
                .pb_expect("Failed to create /bin directory");
            let sh_path = fs_root.join("bin/sh");
            if !sh_path.exists() {
                // Create a simple shell script that acts as sh
                let sh_script = r#"#!/system/bin/sh
# Simple sh replacement for Android/PRoot environment
# Forward all arguments to the system shell
exec /system/bin/sh "$@"
"#;
                fs::write(&sh_path, sh_script)
                    .pb_expect("Failed to create /bin/sh");
                
                #[cfg(unix)]
                {
                    let _ = fs::set_permissions(&sh_path, fs::Permissions::from_mode(0o755));
                }
            }
        }));
    }
    None
}

fn install_dependencies(options: &SetupOptions) -> StageOutput {
    let SetupOptions {
        mpsc_sender,
        android_app: _,
    } = options;

    let context = get_application_context();
    let distribution = context.local_config.distribution.name.clone();
    let (check, install, _launch) = context.local_config.command.get_effective_commands(&distribution);

    let installed = move || {
        ArchProcess::exec(&check)
            .wait()
            .pb_expect("Failed to check whether the installation target is installed")
            .success()
    };

    if installed() {
        return None;
    }

    let mpsc_sender = mpsc_sender.clone();
    let distribution_clone = distribution.clone();
    return Some(thread::spawn(move || {
        // Install dependencies until `check` succeed
        loop {
            // Remove lock file (works for both pacman and xbps)
            match distribution_clone.as_str() {
                "void" => ArchProcess::exec_with_panic_on_error("rm -f /var/db/xbps/.xbps_*"),
                _ => ArchProcess::exec_with_panic_on_error("rm -f /var/lib/pacman/db.lck"),
            }
            
            ArchProcess::exec(&install).with_log(|it| {
                mpsc_sender
                    .send(SetupMessage::Progress(it))
                    .pb_expect("Failed to send log message");
            });
            if installed() {
                break;
            }
        }
    }));
}

fn setup_firefox_config(_: &SetupOptions) -> StageOutput {
    let context = get_application_context();
    let distribution = context.local_config.distribution.name.clone();
    
    let fs_root = match distribution.as_str() {
        "void" => VOID_FS_ROOT,
        "alpine" => ALPINE_FS_ROOT,
        _ => ARCH_FS_ROOT,
    };
    
    // Create the Firefox root directory if it doesn't exist
    let firefox_root = format!("{}/usr/lib/firefox", fs_root);
    let _ = fs::create_dir_all(&firefox_root).pb_expect("Failed to create Firefox root directory");

    // Create the defaults/pref directory
    let pref_dir = format!("{}/defaults/pref", firefox_root);
    let _ = fs::create_dir_all(&pref_dir).pb_expect("Failed to create Firefox pref directory");

    // Create autoconfig.js in defaults/pref
    let autoconfig_js = r#"pref("general.config.filename", "localdesktop.cfg");
pref("general.config.obscure_value", 0);
"#;

    let _ = fs::write(format!("{}/autoconfig.js", pref_dir), autoconfig_js)
        .pb_expect("Failed to write Firefox autoconfig.js");

    // Create localdesktop.cfg in the Firefox root directory
    let firefox_cfg = r#"// Auto updated by Local Desktop on each startup, do not edit manually
defaultPref("media.cubeb.sandbox", false);
defaultPref("security.sandbox.content.level", 0);
"#; // It is required that the first line of this file is a comment, even if you have nothing to comment. Docs: https://support.mozilla.org/en-US/kb/customizing-firefox-using-autoconfig

    let _ = fs::write(format!("{}/localdesktop.cfg", firefox_root), firefox_cfg)
        .pb_expect("Failed to write Firefox configuration");

    None
}

fn fix_xkb_symlink(options: &SetupOptions) -> StageOutput {
    let context = get_application_context();
    let distribution = context.local_config.distribution.name.clone();
    
    let fs_root_str = match distribution.as_str() {
        "void" => VOID_FS_ROOT,
        "alpine" => ALPINE_FS_ROOT,
        _ => ARCH_FS_ROOT,
    };
    
    let fs_root = Path::new(fs_root_str);
    let xkb_path = fs_root.join("usr/share/X11/xkb");
    let mpsc_sender = options.mpsc_sender.clone();

    if let Ok(meta) = fs::symlink_metadata(&xkb_path) {
        if meta.file_type().is_symlink() {
            if let Ok(target) = fs::read_link(&xkb_path) {
                if target.is_absolute() {
                    log::info!(
                        "Absolute symlink target detected: {} -> {}. This is a problem because libxkbcommon is loaded in NDK, whose / is not Arch FS root!",
                        xkb_path.display(),
                        target.display()
                    );
                    // Compute the relative path from /usr/share/X11/xkb to /usr/share/xkeyboard-config-2
                    // Both are inside the chroot, so strip the fs_root prefix
                    let xkb_inside = Path::new("/usr/share/X11/xkb");
                    let target_inside = Path::new("/usr/share/xkeyboard-config-2");
                    let rel_target = diff_paths(target_inside, xkb_inside.parent().unwrap())
                        .unwrap_or_else(|| target_inside.to_path_buf());
                    log::info!(
                        "Fixing with new relative symlink: {} -> {}",
                        xkb_path.display(),
                        rel_target.display()
                    );
                    // Remove the old symlink
                    let _ = fs::remove_file(&xkb_path);
                    // Create the new relative symlink
                    if let Err(e) = symlink(&rel_target, &xkb_path) {
                        mpsc_sender
                            .send(SetupMessage::Error(format!(
                                "Failed to create relative symlink for xkb: {}",
                                e
                            )))
                            .unwrap_or(());
                    }
                }
            }
        }
    }
    None
}

pub fn setup(android_app: AndroidApp) -> PolarBearBackend {
    let (sender, receiver) = mpsc::channel();
    let progress = Arc::new(Mutex::new(0));

    let options = SetupOptions {
        android_app,
        mpsc_sender: sender.clone(),
    };

    let stages: Vec<SetupStage> = vec![
        Box::new(setup_linux_fs),               // Step 1. Setup Linux FS (extract) - now supports both Arch and Void
        Box::new(simulate_linux_sysdata_stage), // Step 2. Simulate Linux system data
        Box::new(install_dependencies),         // Step 3. Install dependencies
        Box::new(setup_firefox_config),         // Step 4. Setup Firefox config
        Box::new(fix_xkb_symlink),              // Step 5. Fix xkb symlink (last)
    ];

    let handle_stage_error = |e: Box<dyn std::any::Any + Send>, sender: &Sender<SetupMessage>| {
        let error_msg = if let Some(e) = e.downcast_ref::<String>() {
            format!("Stage execution failed: {}", e)
        } else if let Some(e) = e.downcast_ref::<&str>() {
            format!("Stage execution failed: {}", e)
        } else {
            "Stage execution failed: Unknown error".to_string()
        };
        sender.send(SetupMessage::Error(error_msg)).unwrap_or(());
    };

    let fully_installed = 'outer: loop {
        for (i, stage) in stages.iter().enumerate() {
            if let Some(handle) = stage(&options) {
                let progress_clone = progress.clone();
                let sender_clone = sender.clone();
                thread::spawn(move || {
                    let progress = progress_clone;
                    let progress_value = ((i) as u16 * 100 / stages.len() as u16) as u16;
                    *progress.lock().unwrap() = progress_value;

                    // Wait for the current stage to finish
                    if let Err(e) = handle.join() {
                        handle_stage_error(e, &sender_clone);
                        return;
                    }

                    // Process the remaining stages in the same loop
                    for (j, next_stage) in stages.iter().enumerate().skip(i + 1) {
                        let progress_value = ((j) as u16 * 100 / stages.len() as u16) as u16;
                        *progress.lock().unwrap() = progress_value;
                        if let Some(next_handle) = next_stage(&options) {
                            if let Err(e) = next_handle.join() {
                                handle_stage_error(e, &sender_clone);
                                return;
                            }

                            // Increment progress and send it
                            let next_progress_value =
                                ((j + 1) as u16 * 100 / stages.len() as u16) as u16;
                            *progress.lock().unwrap() = next_progress_value;
                        }
                    }

                    // All stages are done, we need to replace the WebviewBackend with the WaylandBackend
                    // Or, easier, just restart the whole app
                    *progress.lock().unwrap() = 100;
                    sender_clone
                        .send(SetupMessage::Progress(
                            "Installation finished, please restart the app".to_string(),
                        ))
                        .pb_expect("Failed to send installation finished message");
                });

                // Setup is still running in the background, but we need to return control
                // so that the main thread can continue to report progress to the user
                break 'outer false;
            }
        }

        // All stages were done previously, no need to wait for anything
        break 'outer true;
    };

    if fully_installed {
        PolarBearBackend::Wayland(WaylandBackend {
            compositor: Compositor::build().pb_expect("Failed to build compositor"),
            graphic_renderer: None,
            clock: Clock::new(),
            key_counter: 0,
            scale_factor: 1.0,
        })
    } else {
        PolarBearBackend::WebView(WebviewBackend::build(receiver, progress))
    }
}

/// Verify that package manager tools exist in the rootfs after extraction
fn verify_package_manager_tools(fs_root: &Path, distribution: &str, mpsc_sender: &Sender<SetupMessage>) {
    mpsc_sender
        .send(SetupMessage::Progress(
            format!("Verifying {} package manager tools...", distribution).to_string(),
        ))
        .unwrap_or(());

    log::info!("=== {} PACKAGE MANAGER VERIFICATION STARTED ===", distribution.to_uppercase());
    log::info!("Rootfs path: {:?}", fs_root);
    log::info!("Rootfs exists: {}", fs_root.exists());
    
    if !fs_root.exists() {
        log::error!("❌ CRITICAL: Rootfs directory does not exist!");
        return;
    }

    match distribution {
        "void" => {
            let xbps_query_path = fs_root.join("usr/bin/xbps-query");
            let xbps_install_path = fs_root.join("usr/bin/xbps-install");
            
            log::info!("=== VOID LINUX PACKAGE MANAGER VERIFICATION ===");
            log::info!("xbps-query exists: {}", xbps_query_path.exists());
            log::info!("xbps-install exists: {}", xbps_install_path.exists());
            
            if xbps_query_path.exists() {
                if let Ok(metadata) = std::fs::metadata(&xbps_query_path) {
                    log::info!("xbps-query permissions: {:?}", metadata.permissions());
                    log::info!("xbps-query size: {} bytes", metadata.len());
                } else {
                    log::warn!("Failed to get xbps-query metadata");
                }
            }
            
            if xbps_install_path.exists() {
                if let Ok(metadata) = std::fs::metadata(&xbps_install_path) {
                    log::info!("xbps-install permissions: {:?}", metadata.permissions());
                    log::info!("xbps-install size: {} bytes", metadata.len());
                } else {
                    log::warn!("Failed to get xbps-install metadata");
                }
            }
            
            log::info!("=== SCANNING /usr/bin/ FOR XBPS TOOLS ===");
            if let Ok(entries) = std::fs::read_dir(fs_root.join("usr/bin")) {
                let mut xbps_tools = Vec::new();
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.starts_with("xbps") {
                        xbps_tools.push(name.clone());
                        log::info!("Found XBPS tool: {}", name);
                    }
                }
                log::info!("Total XBPS tools found: {}", xbps_tools.len());
                if xbps_tools.is_empty() {
                    log::error!("❌ NO XBPS TOOLS FOUND IN /usr/bin/ - This confirms the hypothesis!");
                } else {
                    log::info!("✅ XBPS tools found: {:?}", xbps_tools);
                }
            } else {
                log::error!("Failed to read /usr/bin/ directory");
            }
            
            let other_paths = ["bin", "sbin", "usr/sbin"];
            for path in &other_paths {
                let search_path = fs_root.join(path);
                if let Ok(entries) = std::fs::read_dir(&search_path) {
                    for entry in entries.flatten() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if name.starts_with("xbps") {
                            log::info!("Found XBPS tool in {}: {}", path, name);
                        }
                    }
                }
            }
        },
        "alpine" => {
            let apk_path = fs_root.join("sbin/apk");
            
            log::info!("=== ALPINE LINUX PACKAGE MANAGER VERIFICATION ===");
            log::info!("Primary APK path: {:?}", apk_path);
            log::info!("apk exists: {}", apk_path.exists());
            
            let sbin_dir = fs_root.join("sbin");
            log::info!("/sbin directory exists: {}", sbin_dir.exists());
            
            if apk_path.exists() {
                if let Ok(metadata) = std::fs::metadata(&apk_path) {
                    log::info!("✅ apk permissions: {:?}", metadata.permissions());
                    log::info!("✅ apk size: {} bytes", metadata.len());
                    log::info!("✅ apk is_file: {}", metadata.is_file());
                    
                    // Try to read the file to verify it's accessible
                    match std::fs::File::open(&apk_path) {
                        Ok(_) => log::info!("✅ apk file is readable"),
                        Err(e) => log::error!("❌ apk file is not readable: {}", e),
                    }
                } else {
                    log::warn!("❌ Failed to get apk metadata");
                }
            } else {
                log::error!("❌ APK TOOL NOT FOUND AT EXPECTED LOCATION: {:?}", apk_path);
            }
            
            log::info!("=== COMPREHENSIVE DIRECTORY SCAN FOR APK TOOLS ===");
            let search_paths = ["sbin", "bin", "usr/bin", "usr/sbin", "usr/local/bin", "usr/local/sbin"];
            let mut total_apk_tools = 0;
            
            for path in &search_paths {
                let search_path = fs_root.join(path);
                log::info!("Scanning directory: {:?} (exists: {})", search_path, search_path.exists());
                
                if let Ok(entries) = std::fs::read_dir(&search_path) {
                    let mut path_apk_tools = Vec::new();
                    let mut all_files = Vec::new();
                    
                    for entry in entries.flatten() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        all_files.push(name.clone());
                        
                        if name.starts_with("apk") || name.contains("apk") {
                            path_apk_tools.push(name.clone());
                            total_apk_tools += 1;
                            log::info!("✅ Found APK-related tool in {}: {}", path, name);
                            
                            let tool_path = search_path.join(&name);
                            if let Ok(metadata) = std::fs::metadata(&tool_path) {
                                log::info!("   - Size: {} bytes, Executable: {}", 
                                    metadata.len(), 
                                    metadata.permissions().mode() & 0o111 != 0);
                            }
                        }
                    }
                    
                    if path_apk_tools.is_empty() {
                        log::info!("No APK tools in {}, total files: {}", path, all_files.len());
                        if *path == "sbin" && all_files.len() < 10 {
                            log::info!("Files in /sbin: {:?}", all_files);
                        }
                    } else {
                        log::info!("APK tools in {}: {:?}", path, path_apk_tools);
                    }
                } else {
                    log::warn!("Cannot read directory: {:?}", search_path);
                }
            }
            
            log::info!("=== ADVANCED APK TOOL LOCATION DEBUGGING ===");
            
            log::info!("Attempting to execute 'which apk' within rootfs...");
            
            // Try to find apk using different methods
            let potential_apk_locations = [
                "bin/apk", "sbin/apk", "usr/bin/apk", "usr/sbin/apk", 
                "usr/local/bin/apk", "usr/local/sbin/apk", "system/bin/apk"
            ];
            
            for location in &potential_apk_locations {
                let apk_path = fs_root.join(location);
                if apk_path.exists() {
                    log::info!("✅ FOUND APK at: /{}", location);
                    if let Ok(metadata) = std::fs::metadata(&apk_path) {
                        log::info!("   - File size: {} bytes", metadata.len());
                        log::info!("   - Permissions: {:o}", metadata.permissions().mode());
                        log::info!("   - Is executable: {}", metadata.permissions().mode() & 0o111 != 0);
                        log::info!("   - Is file: {}", metadata.is_file());
                        log::info!("   - Is symlink: {}", metadata.file_type().is_symlink());
                        
                        if metadata.file_type().is_symlink() {
                            if let Ok(target) = std::fs::read_link(&apk_path) {
                                log::info!("   - Symlink target: {:?}", target);
                            }
                        }
                        
                        // Try to read first few bytes to verify it's a valid binary
                        if let Ok(mut file) = std::fs::File::open(&apk_path) {
                            let mut buffer = [0u8; 16];
                            if let Ok(bytes_read) = std::io::Read::read(&mut file, &mut buffer) {
                                log::info!("   - First {} bytes: {:02x?}", bytes_read, &buffer[..bytes_read]);
                                
                                if bytes_read >= 4 && buffer[0] == 0x7f && buffer[1] == 0x45 && buffer[2] == 0x4c && buffer[3] == 0x46 {
                                    log::info!("   - ✅ Valid ELF binary detected");
                                } else {
                                    log::warn!("   - ⚠️  Not a standard ELF binary");
                                }
                            }
                        }
                    }
                }
            }
            
            log::info!("=== PATH ENVIRONMENT ANALYSIS ===");
            log::info!("Standard Alpine Linux PATH should be: /usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin");
            
            let path_dirs = ["usr/local/sbin", "usr/local/bin", "usr/sbin", "usr/bin", "sbin", "bin"];
            for dir in &path_dirs {
                let dir_path = fs_root.join(dir);
                log::info!("PATH directory /{} exists: {}", dir, dir_path.exists());
                if dir_path.exists() {
                    if let Ok(entries) = std::fs::read_dir(&dir_path) {
                        let count = entries.count();
                        log::info!("   - Contains {} files/directories", count);
                    }
                }
            }
            
            // Try to simulate 'which apk' by checking each PATH directory
            log::info!("=== SIMULATING 'which apk' COMMAND ===");
            let mut found_apk_in_path = false;
            for dir in &path_dirs {
                let apk_in_dir = fs_root.join(dir).join("apk");
                if apk_in_dir.exists() {
                    log::info!("✅ 'which apk' would find: /{}/apk", dir);
                    found_apk_in_path = true;
                    break;
                }
            }
            
            if !found_apk_in_path {
                log::error!("❌ 'which apk' would return: command not found");
                log::error!("APK tool is not in any standard PATH directory");
            }
            
            log::info!("=== ROOTFS INTEGRITY VERIFICATION ===");
            let critical_dirs = ["etc", "lib", "usr", "var", "tmp"];
            for dir in &critical_dirs {
                let dir_path = fs_root.join(dir);
                log::info!("Critical directory /{} exists: {}", dir, dir_path.exists());
            }
            
            let alpine_files = ["etc/alpine-release", "etc/apk/repositories", "lib/apk"];
            for file in &alpine_files {
                let file_path = fs_root.join(file);
                log::info!("Alpine-specific path /{} exists: {}", file, file_path.exists());
            }
            
            log::info!("=== APK VERIFICATION SUMMARY ===");
            log::info!("Total APK-related tools found: {}", total_apk_tools);
            if total_apk_tools == 0 {
                log::error!("❌ CRITICAL: NO APK TOOLS FOUND ANYWHERE IN ROOTFS!");
                log::error!("This confirms the Alpine Linux rootfs is incomplete or corrupted.");
                log::error!("Possible causes:");
                log::error!("  1. Termux Alpine rootfs extraction failed");
                log::error!("  2. APK tools are in non-standard locations");
                log::error!("  3. Rootfs is missing essential Alpine Linux components");
            } else {
                log::info!("✅ APK tools detected in rootfs");
            }
        },
        _ => {
            let pacman_path = fs_root.join("usr/bin/pacman");
            log::info!("=== ARCH LINUX PACKAGE MANAGER VERIFICATION ===");
            log::info!("pacman exists: {}", pacman_path.exists());
            
            if pacman_path.exists() {
                if let Ok(metadata) = std::fs::metadata(&pacman_path) {
                    log::info!("pacman permissions: {:?}", metadata.permissions());
                    log::info!("pacman size: {} bytes", metadata.len());
                }
            }
            
            if let Ok(entries) = std::fs::read_dir(fs_root.join("usr/bin")) {
                let mut pacman_tools = Vec::new();
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.starts_with("pacman") {
                        pacman_tools.push(name.clone());
                        log::info!("Found pacman tool: {}", name);
                    }
                }
                log::info!("Total pacman tools found: {}", pacman_tools.len());
            }
        }
    }
    
    log::info!("=== PACKAGE MANAGER VERIFICATION COMPLETE ===");
}
