use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use autoschematic_core::macros::FieldTypes;
use autoschematic_macros::FieldTypes;
use documented::{Documented, DocumentedFields};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Debug, Deserialize, Clone, Documented, DocumentedFields, FieldTypes)]
#[serde(deny_unknown_fields)]
/// A RemoteFsMount represents a set of files to
/// sync to/from a remote server on the host.
pub struct RemoteFsMount {
    /// Directories under this mountpoint to sync.
    pub dirs: Option<Vec<PathBuf>>,
    /// Individual files under this mountpoint to sync.
    pub files: Option<Vec<PathBuf>>,
    // TODO work out if globs are relative or absolute??
    /// A set of globs, absolute paths with e.g. **/* and * that will be used to filter files within this mount.
    /// Only paths that match the globs will be included.
    pub globs: Option<Vec<String>>,
    /// UNIX user id.
    pub uid: Option<u32>,
    /// UNIX group id.
    pub gid: Option<u32>,
    /// UNIX file permissions (Dont forget, ron supports octal with `mode: 0o755` !).
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

use std::ops::Not;

#[derive(Serialize, Deserialize, Clone, Debug, Documented, DocumentedFields, FieldTypes)]
#[serde(deny_unknown_fields)]
/// RemoteFsHook represents a shell hook to execute on the remote server before or after operating on a file.
/// Execution of these hooks is always an explicit operation in the plan or apply output.
pub struct RemoteFsHook {
    /// The working directory in which to execute the hook.
    pub work_dir: Option<PathBuf>,
    /// The shell command to execute. Usually runs under sh -c on the remote host.
    pub shell: String,
    /// If true, the hook will not cause the entire workflow to fail if it returns nonzero.
    #[serde(skip_serializing_if = "<&bool>::not")]
    #[serde(default)]
    pub ignore_error: bool,
}

#[derive(Serialize, Deserialize, Clone, Documented, DocumentedFields, FieldTypes)]
#[serde(deny_unknown_fields)]
/// RemoteFsHost defines the parameters of a host to connect to.
pub struct RemoteFsHost {
    /// The UNIX username to connect with.
    pub username: String,
    /// The remote SSH port to connect to.
    pub port: u16,
    /// A set of RemoteFsMount objects. Multiple points within a host's
    /// remote filesystem can be mounted with multiple RemoteFsMounts.
    /// Mounts can also contain hooks and permission settings.
    pub mounts: Vec<RemoteFsMount>,
    /// The path to the SSH private key with which to connect to the remote host.
    pub ssh_private_key_path: PathBuf,
    /// If specified, use this SSH config file instead of the default at .ssh/config.
    pub ssh_config_path: Option<PathBuf>,
}

#[derive(Serialize, Deserialize, Clone, Default, Documented, DocumentedFields, FieldTypes)]
#[serde(deny_unknown_fields)]
///The main RemoteFsConnector config block.
pub struct RemoteFsConfig {
    /// A map of hosts => RemoteFsHost config blocks.
    pub hosts: HashMap<String, RemoteFsHost>,
}
