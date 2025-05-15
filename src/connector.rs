use std::path::{Path, PathBuf};

use anyhow::bail;
use autoschematic_core::{
    connector::{
        Connector, ConnectorOp, ConnectorOutbox, GetResourceOutput, OpExecOutput, OpPlanOutput,
        Resource, ResourceAddress,
    },
    connector_op,
    diag::DiagnosticOutput,
    get_resource_output, op_exec_output,
    util::{RON, ron_check_syntax},
};
use tokio::sync::Mutex;

use std::{
    io::{Read, Write},
    sync::Arc,
};

use async_trait::async_trait;
use dashmap::DashMap;
use glob_match::glob_match;
use remotefs::{
    RemoteFs,
    fs::{Metadata, UnixPex},
};
use remotefs_ssh::{ScpFs, SshKeyStorage, SshOpts};
use serde::{Deserialize, Serialize};

use tempfile::NamedTempFile;

use crate::{
    addr::RemoteFsPath,
    config::{RemoteFsConfig, RemoteFsHost},
    resource::FileContents,
};

pub struct ConnectorSshKeyStorage {
    key_path: PathBuf,
}

impl ConnectorSshKeyStorage {
    fn from_str(private_key: &str) -> Result<Self, anyhow::Error> {
        let key_file = NamedTempFile::new()?;

        std::fs::write(key_file.path(), private_key)?;

        Ok(Self {
            key_path: key_file.path().to_path_buf(),
        })
    }
    fn from_path(private_key_path: &Path) -> Result<Self, anyhow::Error> {
        Ok(Self {
            key_path: private_key_path.to_path_buf(),
        })
    }
}

impl SshKeyStorage for ConnectorSshKeyStorage {
    fn resolve(&self, _host: &str, _username: &str) -> Option<std::path::PathBuf> {
        Some(self.key_path.clone())
    }
}

pub struct RemoteFsConnector {
    // client: ScpFs,
    client_cache: DashMap<String, Arc<Mutex<ScpFs>>>,
    config: RemoteFsConfig,
    prefix: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum RemoteFsConnectorOp {
    Copy,
    Delete,
}

impl ConnectorOp for RemoteFsConnectorOp {
    fn to_string(&self) -> Result<String, anyhow::Error> {
        Ok(ron::to_string(self)?)
    }

    fn from_str(s: &str) -> Result<Self, anyhow::Error>
    where
        Self: Sized,
    {
        Ok(ron::from_str(s)?)
    }
}

impl RemoteFsConnector {
    fn get_client(&self, hostname: &str) -> Result<Arc<Mutex<ScpFs>>, anyhow::Error> {
        if self.client_cache.contains_key(hostname) {
            let client = self.client_cache.get(hostname).unwrap();
            Ok(client.clone())
        } else {
            let Some(host_config) = &self.config.hosts.get(hostname) else {
                bail!("Host {} not in config", hostname);
            };

            let mut sshopts = SshOpts::new(hostname);
            if let Some(ssh_config_path) = &host_config.ssh_config_path {
                sshopts = sshopts
                    .config_file(&ssh_config_path, remotefs_ssh::SshConfigParseRule::empty());
            }

            sshopts = sshopts
                .key_storage(Box::new(ConnectorSshKeyStorage::from_path(
                    &host_config.ssh_private_key_path,
                )?))
                .into();

            let mut client: remotefs_ssh::ScpFs = sshopts.into();

            client.connect()?;

            self.client_cache
                .insert(hostname.to_string(), Arc::new(Mutex::new(client)));

            let client = self.client_cache.get(hostname).unwrap();
            Ok(client.clone())
        }
    }

    fn matches_any_globs(path: &Path, globs: &Vec<String>) -> bool {
        // The empty globset is equivalent to ["**/*"]
        if globs.len() == 0 {
            return true;
        }
        for glob in globs {
            if glob_match(&glob, &path.to_string_lossy()) {
                return true;
            }
        }
        return false;
    }

    // Hmm.. ok, if we have globs like:
    // globs = ["/etc/cron/**/*"]
    // and we start at "/",
    // we need to somehow optimize away searching through
    // /bin, /tmp, etc...
    fn list_recursive(
        client: &mut ScpFs,
        dir: &Path,
        globs: &Vec<String>,
    ) -> Result<Vec<remotefs::File>, anyhow::Error> {
        let mut results = Vec::new();

        tracing::debug!("list_recursive: {:?}", dir);

        if client.exists(dir)? {
            for file in client.list_dir(dir)? {
                if file.is_dir() {
                    results.append(&mut Self::list_recursive(client, &file.path, globs)?);
                } else {
                    // TODO are globs absolute or relative?
                    results.push(file);
                    // if RemoteFsConnector::matches_any_globs(&file.path, globs) {
                    //     results.push(file);
                    // }
                }
            }
        }

        Ok(results)
    }
}

#[async_trait]
impl Connector for RemoteFsConnector {
    async fn new(
        name: &str,
        prefix: &Path,
        outbox: ConnectorOutbox,
    ) -> Result<Box<dyn Connector>, anyhow::Error>
    where
        Self: Sized,
    {
        let cfg_path = prefix.to_path_buf().join("remotefs/config.ron");

        let cfg_body = if cfg_path.is_file() {
            std::fs::read_to_string(cfg_path)?
        } else {
            bail!(
                "RemoteFs connector config not found! Tried looking in {:?}",
                cfg_path
            );
        };

        let config = RON.from_str(&cfg_body)?;

        Ok(Box::new(RemoteFsConnector {
            client_cache: DashMap::new(),
            config: config,
            prefix: prefix.to_path_buf(),
        }))
    }

    async fn filter(&self, addr: &Path) -> Result<bool, anyhow::Error> {
        let addr = RemoteFsPath::from_path(addr);
        match addr {
            Ok(Some(addr)) => {
                if self.config.hosts.contains_key(&addr.hostname) {
                    return Ok(true);
                } else {
                    return Ok(false);
                }
            }
            _ => {
                return Ok(false);
            }
        }
    }

    async fn list(&self, subpath: &Path) -> Result<Vec<PathBuf>, anyhow::Error> {
        // let hostnames: Vec<String> = self.config.keys().map(|h| h.clone()).collect_vec();

        let mut results: Vec<PathBuf> = Vec::new();
        for hostname in self.config.hosts.keys() {
            let Some(host) = &self.config.hosts.get(hostname) else {
                continue;
            };
            let client = self.get_client(&hostname)?;
            let client = &mut *client.lock().await;

            for mount in &host.mounts {
                println!("mount: {:?}, subpath: {:?}", &mount.path, subpath);
                // if mount.path.starts_with(subpath) {
                let listing = RemoteFsConnector::list_recursive(client, &mount.path, &mount.globs)?;
                for file in listing {
                    let path = if file.path.is_absolute() {
                        file.path.strip_prefix("/").unwrap()
                    } else {
                        &file.path
                    };
                    results.push(PathBuf::from("remotefs").join(&hostname).join(path));
                }
                // }
            }
        }
        Ok(results)
    }

    async fn get(&self, addr: &Path) -> Result<Option<GetResourceOutput>, anyhow::Error> {
        let addr = RemoteFsPath::from_path(addr)?;
        match addr {
            Some(addr) => {
                let remote_path = PathBuf::from("/").join(&addr.path);
                println!("GET: {:?} -> {:?}", &addr.path, &remote_path);
                // self.client.remove_file(&remote_path)?;
                let client = self.get_client(&addr.hostname)?;
                let client = &mut *client.lock().await;
                if client.exists(&remote_path)? {
                    let mut read_stream = client.open(&remote_path)?;
                    let mut body = String::new();
                    read_stream.read_to_string(&mut body)?;
                    get_resource_output!(FileContents { contents: body })
                } else {
                    Ok(None)
                }
            }
            None => Ok(None),
        }
    }

    async fn plan(
        &self,
        addr: &Path,
        current: Option<String>,
        desired: Option<String>,
    ) -> Result<Vec<OpPlanOutput>, anyhow::Error> {
        tracing::info!("plan {:?} -? {:?}", current, desired);
        let addr = RemoteFsPath::from_path(addr)?;

        match addr {
            Some(addr) => {
                match (current, desired) {
                    (None, None) => Ok(Vec::new()),
                    (Some(_), None) => {
                        // RemoteFs delete

                        Ok(vec![connector_op!(
                            RemoteFsConnectorOp::Delete,
                            format!(
                                "Delete remote file at {}/{}",
                                addr.hostname,
                                addr.path.to_string_lossy()
                            )
                        )])
                    }
                    (Some(_), Some(_)) => Ok(vec![connector_op!(
                        RemoteFsConnectorOp::Copy,
                        format!(
                            "Modify remote file at {}/{}",
                            addr.hostname,
                            addr.path.to_string_lossy()
                        )
                    )]),
                    (None, Some(_)) => {
                        //RemoteFs push
                        Ok(vec![connector_op!(
                            RemoteFsConnectorOp::Copy,
                            format!(
                                "Create new remote file at {}/{}",
                                addr.hostname,
                                addr.path.to_string_lossy()
                            )
                        )])
                    }
                }
            }
            None => Ok(vec![]),
        }
    }

    async fn op_exec(&self, addr: &Path, op: &str) -> Result<OpExecOutput, anyhow::Error> {
        let op = RemoteFsConnectorOp::from_str(op)?;
        let Some(addr) = RemoteFsPath::from_path(addr)? else {
            bail!("RemoteFsConnector::op_exec(): invalid path: {:?}", addr)
        };

        match op {
            RemoteFsConnectorOp::Copy => {
                // let size: u64 = contents.contents.len().try_into()?;
                // self.client.session().unwrap().scp_send(&addr.path, mode, size, None);
                //
                // thinking out loud:
                // suppose we have a remotefs connector at a prefix, like ./autoschematic/tainan_office/remotefs/server.com/etc/locale.conf
                // and addr = ./etc/locale.conf
                // then the path on the remote host is just Path::from("/").join(addr);
                // ...and the path on the local host
                let local_path = self.prefix.join(addr.to_path_buf());
                let remote_path = PathBuf::from("/").join(&addr.path);
                println!("COPY: {:?} -> {:?}", &local_path, &remote_path);
                println!("COPY: pwd = {:?}", &std::env::current_dir()?);
                // self.client.copy(&addr.path, &remote_path)?;
                let client = self.get_client(&addr.hostname)?;
                let client = &mut *client.lock().await;
                println!("COPY: pwd = {:?}", client.pwd()?);

                let Some(host) = self.config.hosts.get(&addr.hostname) else {
                    bail!("Host {} not in config", addr.hostname);
                };
                let mounts = &host.mounts;
                for mount in mounts.iter().rev() {
                    if remote_path.starts_with(&mount.path) {
                        // yeah, right here is fine, thanks
                        // (pick the last mount that matches the globs - on the
                        // assumption that mounts are listed in order of most general -> most specific)
                        let size = std::fs::metadata(&local_path)?.len();
                        let metadata = Metadata {
                            accessed: None,
                            created: None,
                            modified: None,
                            uid: mount.uid,
                            gid: mount.gid,
                            mode: mount.mode.map(|m| UnixPex::from(m)),
                            size: size,
                            symlink: None,
                            file_type: remotefs::fs::FileType::File,
                        };
                        let mut stream = client.create(&remote_path, &metadata)?;
                        let buf = tokio::fs::read(&local_path).await?;
                        stream.write_all(&buf)?;
                        return op_exec_output!(format!(
                            "Wrote remote file at {}/{}",
                            addr.hostname,
                            addr.path.to_string_lossy()
                        ));
                    }
                }
                let size = std::fs::metadata(&local_path)?.len();
                let metadata = Metadata {
                    accessed: None,
                    created: None,
                    modified: None,
                    uid: None,
                    gid: None,
                    mode: None,
                    size: size,
                    symlink: None,
                    file_type: remotefs::fs::FileType::File,
                };

                let mut stream = client.create(&remote_path, &metadata)?;
                let buf = tokio::fs::read(&local_path).await?;
                stream.write_all(&buf)?;

                return op_exec_output!(format!(
                    "Wrote remote file at {}/{}",
                    addr.hostname,
                    addr.path.to_string_lossy()
                ));
                // let metadata = Metadata { accessed: None, created: None, gid:  mode: todo!(), modified: todo!(), size: todo!(), symlink: todo!(), file_type: todo!(), uid: todo!() }};
                // let stream = client.create(&remote_path);
                // client.copy(&local_path, &remote_path)?;
            }
            RemoteFsConnectorOp::Delete => {
                let remote_path = PathBuf::from("/").join(&addr.path);
                println!("DELETE: {:?} -> {:?}", &addr.path, &remote_path);
                // self.client.remove_file(&remote_path)?;
                let client = self.get_client(&addr.hostname)?;
                let client = &mut *client.lock().await;
                client.remove_file(&remote_path)?;
                return op_exec_output!(format!(
                    "Deleted remote file at {}/{}",
                    addr.hostname,
                    addr.path.to_string_lossy()
                ));
            }
        }
    }

    async fn eq(&self, addr: &Path, a: &str, b: &str) -> Result<bool, anyhow::Error> {
        Ok(a == b)
    }

    async fn diag(&self, addr: &Path, a: &str) -> Result<DiagnosticOutput, anyhow::Error> {
        if addr == PathBuf::from("remotefs/config.ron") {
            ron_check_syntax::<RemoteFsHost>(a)
        } else {
            Ok(DiagnosticOutput {
                diagnostics: Vec::new(),
            })
        }
    }
}
