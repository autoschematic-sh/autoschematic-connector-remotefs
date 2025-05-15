use std::path::{Path, PathBuf};

use autoschematic_core::connector::ResourceAddress;


#[derive(Debug, Clone)]
pub struct RemoteFsPath {
    pub hostname: String,
    pub path: PathBuf,
}

impl ResourceAddress for RemoteFsPath {
    fn to_path_buf(&self) -> std::path::PathBuf {
        // From an absolute (remote) path E.G. /etc/crontab ,
        // form the relative path from ./remotefs/psychlone.xyz/etc/crontab
        let path = if self.path.is_absolute() {
            self.path.strip_prefix("/").unwrap()
        } else {
            &self.path
        };
        PathBuf::from("remotefs")
            .join(self.hostname.clone())
            .join(path)
    }

    fn from_path(path: &Path) -> Result<Option<Self>, anyhow::Error> {
        let path = if path.is_absolute() {
            path.strip_prefix("/").unwrap()
        } else {
            path
        };

        let path_components: Vec<&str> = path
            .components()
            .into_iter()
            .map(|s| s.as_os_str().to_str().unwrap())
            .collect();


        // path = "./remotefs/psychlone.xyz/etc/crontab"
        // local_path = "./etc/crontab"
        match path_components[..] {
            ["remotefs", hostname, ..] => {
                let prefix = PathBuf::from("remotefs").join(hostname);
                let local_path = path.strip_prefix(prefix)?;
                Ok(Some(RemoteFsPath {
                    hostname: hostname.to_string(),
                    path: local_path.to_path_buf(),
                }))
            }
            _ => Ok(None),
        }
    }
}