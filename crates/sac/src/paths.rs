use std::env;
use std::path::PathBuf;

pub fn sac_home_dir() -> Option<PathBuf> {
    if let Some(sac_home) = env::var_os("SAC_HOME") {
        return Some(PathBuf::from(sac_home));
    }

    if let Some(xdg_config_home) = env::var_os("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(xdg_config_home).join("sac"));
    }

    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".config").join("sac"))
}

pub fn sac_config_path() -> Option<PathBuf> {
    sac_home_dir().map(|dir| dir.join("config.toml"))
}

pub fn sac_logs_dir() -> Option<PathBuf> {
    sac_home_dir().map(|dir| dir.join("logs"))
}

pub fn sac_log_path_for_pid(pid: u32) -> Option<PathBuf> {
    sac_logs_dir().map(|dir| dir.join(format!("sac-{}.log", pid)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env_lock;
    use std::ffi::OsString;

    fn restore_env(name: &str, value: Option<OsString>) {
        match value {
            Some(value) => unsafe { std::env::set_var(name, value) },
            None => unsafe { std::env::remove_var(name) },
        }
    }

    #[test]
    fn log_path_uses_sac_home_layout() {
        let _guard = test_env_lock();
        let original_sac_home = std::env::var_os("SAC_HOME");
        let root = std::env::temp_dir().join(format!(
            "sac_logs_path_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time went backwards")
                .as_nanos()
        ));

        unsafe {
            std::env::set_var("SAC_HOME", &root);
        }

        let path = sac_log_path_for_pid(4242).unwrap();
        assert_eq!(path, root.join("logs").join("sac-4242.log"));

        restore_env("SAC_HOME", original_sac_home);
    }
}
