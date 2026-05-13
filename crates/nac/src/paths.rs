use std::env;
use std::path::PathBuf;

pub fn nac_home_dir() -> Option<PathBuf> {
    if let Some(nac_home) = env::var_os("NAC_HOME") {
        return Some(PathBuf::from(nac_home));
    }

    if let Some(xdg_config_home) = env::var_os("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(xdg_config_home).join("nac"));
    }

    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".config").join("nac"))
}

pub fn nac_config_path() -> Option<PathBuf> {
    nac_home_dir().map(|dir| dir.join("config.toml"))
}

pub fn nac_logs_dir() -> Option<PathBuf> {
    nac_home_dir().map(|dir| dir.join("logs"))
}

pub fn nac_log_path_for_pid(pid: u32) -> Option<PathBuf> {
    nac_logs_dir().map(|dir| dir.join(format!("nac-{}.log", pid)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TEST_ENV_LOCK;
    use std::ffi::OsString;

    fn restore_env(name: &str, value: Option<OsString>) {
        match value {
            Some(value) => unsafe { std::env::set_var(name, value) },
            None => unsafe { std::env::remove_var(name) },
        }
    }

    #[test]
    fn log_path_uses_nac_home_layout() {
        let _guard = TEST_ENV_LOCK.lock().unwrap();
        let original_nac_home = std::env::var_os("NAC_HOME");
        let root = std::env::temp_dir().join(format!(
            "nac_logs_path_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time went backwards")
                .as_nanos()
        ));

        unsafe {
            std::env::set_var("NAC_HOME", &root);
        }

        let path = nac_log_path_for_pid(4242).unwrap();
        assert_eq!(path, root.join("logs").join("nac-4242.log"));

        restore_env("NAC_HOME", original_nac_home);
    }
}
