use crate::core::config;
use crate::android::utils::application_context::get_application_context;
use smithay::reexports::wayland_server::ListeningSocket;
use std::{error::Error, path::PathBuf};

pub fn bind_socket() -> Result<ListeningSocket, Box<dyn Error>> {
    let context = get_application_context();
    let distribution = context.local_config.distribution.name.clone();
    
    let fs_root = match distribution.as_str() {
        "void" => config::VOID_FS_ROOT,
        "alpine" => config::ALPINE_FS_ROOT,
        _ => config::ARCH_FS_ROOT,
    };
    
    let socket_path = PathBuf::from(fs_root.to_owned() + "/tmp").join(config::WAYLAND_SOCKET_NAME);
    let listener = ListeningSocket::bind_absolute(socket_path)?;
    Ok(listener)
}
