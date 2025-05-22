use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
/// A RemoteFsMount represents a set of files to
/// sync to/from a remote server on the host
pub struct RemoteFsMount {
    /// Directories under this mountpoint to sync
    pub dirs: Option<Vec<PathBuf>>,
    /// Individual files under this mountpoint to sync
    pub files: Option<Vec<PathBuf>>,
    // TODO work out if globs are relative or absolute??
    pub globs: Option<Vec<String>>,
    /// UNIX user id
    pub uid: Option<u32>,
    /// UNIX group id
    pub gid: Option<u32>,
    /// UNIX file permissions (Dont forget, ron supports octal with `mode: 0o755` !)
    pub mode: Option<u32>,
    /// Hooks that are executed before a file in this mount is created, modified, or deleted.
    pub pre_hooks: Option<Vec<RemoteFsHook>>,
    /// Hooks that are executed after a file in this mount is created, modified, or deleted.
    pub post_hooks: Option<Vec<RemoteFsHook>>,
}

impl RemoteFsMount {
    pub fn path_matches_mount(&self, path: &Path) -> bool {
        if let Some(ref files) = self.files {
            for file in files {
                if path == file {
                    return true;
                }
            }
        }

        if let Some(ref dirs) = self.dirs {
            for dir in dirs {
                if path.starts_with(dir) {
                    return true;
                }
            }
        }

        false
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct RemoteFsHook {
    pub work_dir: Option<PathBuf>,
    pub shell: String,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct RemoteFsHost {
    pub username: String,
    pub port: u16,
    pub mounts: Vec<RemoteFsMount>,
    pub ssh_private_key_path: PathBuf,
    pub ssh_config_path: Option<PathBuf>,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct RemoteFsConfig {
    pub hosts: HashMap<String, RemoteFsHost>,
}
