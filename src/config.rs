use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Serialize};


#[derive(Serialize, Deserialize, Clone)]
pub struct RemoteFsMount {
    pub path: PathBuf,
    // TODO work out if globs are relative or absolute??
    pub globs: Vec<String>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub mode: Option<u32>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct RemoteFsHost {
    pub username: String,
    pub port: u16,
    pub mounts: Vec<RemoteFsMount>,
    pub ssh_private_key_path: PathBuf,
    pub ssh_config_path: Option<PathBuf>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct RemoteFsConfig {
    pub hosts: HashMap<String, RemoteFsHost>
}