use std::{
    default,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};
use autoschematic_core::{
    connector::{
        Connector, ConnectorOp, ConnectorOutbox, FilterResponse, GetResourceResponse, OpExecResponse, PlanResponseElement,
        Resource, ResourceAddress,
    },
    connector_op,
    diag::DiagnosticResponse,
    get_resource_response, op_exec_output,
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
use remotefs_ssh::{LibSsh2Session, ScpFs, SshKeyStorage, SshOpts};
use serde::{Deserialize, Serialize};

use tempfile::NamedTempFile;

use crate::{
    addr::RemoteFsPath,
    config::{RemoteFsConfig, RemoteFsHook, RemoteFsHost},
    resource::FileContents,
};

#[derive(Debug)]
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

#[derive(Default)]
pub struct RemoteFsConnector {
    // client: ScpFs,
    client_cache: DashMap<String, Arc<Mutex<ScpFs<LibSsh2Session>>>>,
    config: Mutex<RemoteFsConfig>,
    prefix: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum RemoteFsConnectorOp {
    Copy,
    Delete,
    Exec(RemoteFsHook),
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
    async fn get_client(&self, hostname: &str) -> Result<Arc<Mutex<ScpFs<LibSsh2Session>>>, anyhow::Error> {
        if self.client_cache.contains_key(hostname) {
            let client = self.client_cache.get(hostname).unwrap();
            Ok(client.clone())
        } else {
            let config = self.config.lock().await;
            let Some(host_config) = &config.hosts.get(hostname) else {
                bail!("Host {} not in config", hostname);
            };

            let mut sshopts = SshOpts::new(hostname);
            if let Some(ssh_config_path) = &host_config.ssh_config_path {
                sshopts = sshopts.config_file(ssh_config_path, remotefs_ssh::SshConfigParseRule::empty());
            }

            sshopts = sshopts
                .username(&host_config.username)
                .port(host_config.port)
                .key_storage(Box::new(ConnectorSshKeyStorage::from_path(
                    &host_config.ssh_private_key_path,
                )?));

            let mut client: remotefs_ssh::ScpFs<LibSsh2Session> = sshopts.into();

            client.connect()?;

            self.client_cache.insert(hostname.to_string(), Arc::new(Mutex::new(client)));

            let client = self.client_cache.get(hostname).unwrap();
            Ok(client.clone())
        }
    }

    fn matches_any_globs(path: &Path, globs: &Vec<String>) -> bool {
        // The empty globset is equivalent to ["**/*"]
        if globs.is_empty() {
            return true;
        }
        for glob in globs {
            if glob_match(glob, &path.to_string_lossy()) {
                return true;
            }
        }
        false
    }

    // Hmm.. ok, if we have globs like:
    // globs = ["/etc/cron/**/*"]
    // and we start at "/",
    // we need to somehow optimize away searching through
    // /bin, /tmp, etc...
    fn list_recursive(
        client: &mut ScpFs<LibSsh2Session>,
        dir: &Path,
        globs: &Option<Vec<String>>,
    ) -> Result<Vec<remotefs::File>, anyhow::Error> {
        let mut results = Vec::new();

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

    fn remote_file_exists(
        client: &mut ScpFs<LibSsh2Session>,
        path: &Path,
        globs: &Option<Vec<String>>,
    ) -> Result<bool, anyhow::Error> {
        Ok(client.exists(path)?)
    }
}

#[async_trait]
impl Connector for RemoteFsConnector {
    async fn new(name: &str, prefix: &Path, outbox: ConnectorOutbox) -> Result<Arc<dyn Connector>, anyhow::Error>
    where
        Self: Sized,
    {
        Ok(Arc::new(RemoteFsConnector {
            prefix: prefix.to_path_buf(),
            ..Default::default()
        }))
    }

    async fn init(&self) -> anyhow::Result<()> {
        let cfg_path = self.prefix.to_path_buf().join("remotefs/config.ron");

        let cfg_body = if cfg_path.is_file() {
            std::fs::read_to_string(cfg_path)?
        } else {
            bail!("RemoteFs connector config not found! Tried looking in {:?}", cfg_path);
        };

        let config = RON.from_str(&cfg_body)?;

        self.client_cache.clear();
        *self.config.lock().await = config;

        Ok(())
    }

    async fn filter(&self, addr: &Path) -> Result<FilterResponse, anyhow::Error> {
        let addr = RemoteFsPath::from_path(addr);
        // Alert! Alert!
        // Look at this? filter() isn't a static function anymore!
        // The only solution is to clear connector_cache.filter_cache when we reinit!
        let config = self.config.lock().await;

        match addr {
            Ok(addr) => {
                if config.hosts.contains_key(&addr.hostname) {
                    return Ok(FilterResponse::Resource);
                } else {
                    return Ok(FilterResponse::None);
                }
            }
            _ => {
                return Ok(FilterResponse::None);
            }
        }
    }

    async fn list(&self, subpath: &Path) -> Result<Vec<PathBuf>, anyhow::Error> {
        // let hostnames: Vec<String> = self.config.keys().map(|h| h.clone()).collect_vec();

        let config = self.config.lock().await.clone();

        let mut results: Vec<PathBuf> = Vec::new();
        for hostname in config.hosts.keys() {
            let Some(host) = &config.hosts.get(hostname) else {
                continue;
            };
            let client = self.get_client(hostname).await?;
            let client = &mut *client.lock().await;

            for mount in &host.mounts {
                if let Some(ref dirs) = mount.dirs {
                    for dir in dirs {
                        let listing = RemoteFsConnector::list_recursive(client, dir, &mount.globs)?;
                        for file in listing {
                            let path = if file.path.is_absolute() {
                                file.path.strip_prefix("/").unwrap()
                            } else {
                                &file.path
                            };
                            results.push(PathBuf::from("remotefs").join(hostname).join(path));
                        }
                    }
                }
                if let Some(ref files) = mount.files {
                    for file in files {
                        if RemoteFsConnector::remote_file_exists(client, file, &mount.globs)? {
                            let path = if file.is_absolute() {
                                file.strip_prefix("/").unwrap()
                            } else {
                                &file
                            };
                            results.push(PathBuf::from("remotefs").join(hostname).join(path));
                        }
                    }
                }
                // }
            }
        }
        Ok(results)
    }

    async fn get(&self, addr: &Path) -> Result<Option<GetResourceResponse>, anyhow::Error> {
        let addr = RemoteFsPath::from_path(addr)?;

        let remote_path = PathBuf::from("/").join(&addr.path);
        // self.client.remove_file(&remote_path)?;
        let client = self.get_client(&addr.hostname).await?;
        let client = &mut *client.lock().await;
        if client.exists(&remote_path)? {
            let mut read_stream = client.open(&remote_path)?;
            let mut body: Vec<u8> = Vec::new();
            eprintln!("GET: starting");
            read_stream.read_to_end(&mut body).context("read_to_end")?;
            eprintln!("GET: len {}", body.len());
            get_resource_response!(FileContents { contents: body })
        } else {
            Ok(None)
        }
    }

    async fn plan(
        &self,
        addr: &Path,
        current: Option<Vec<u8>>,
        desired: Option<Vec<u8>>,
    ) -> Result<Vec<PlanResponseElement>, anyhow::Error> {
        let config = self.config.lock().await;

        let addr = RemoteFsPath::from_path(addr)?;

        let remote_path = PathBuf::from("/").join(&addr.path);
        let Some(host) = config.hosts.get(&addr.hostname) else {
            return Ok(Vec::new());
        };

        let mut pre_hooks = Vec::new();
        let mut post_hooks = Vec::new();
        for mount in host.mounts.iter().rev() {
            if mount.path_matches_mount(&remote_path) {
                pre_hooks = mount.pre_hooks.clone().unwrap_or_default();
                post_hooks = mount.post_hooks.clone().unwrap_or_default();
                break;
            }
        }

        let mut res = Vec::new();

        for hook in pre_hooks {
            res.push(connector_op!(
                RemoteFsConnectorOp::Exec(hook.clone()),
                format!("Execute hook: {}", hook.shell)
            ));
        }

        match (current, desired) {
            (None, None) => return Ok(Vec::new()),
            (Some(_), None) => {
                // RemoteFs delete

                res.push(connector_op!(
                    RemoteFsConnectorOp::Delete,
                    format!("Delete remote file at {}/{}", addr.hostname, addr.path.to_string_lossy())
                ));
            }
            (Some(_), Some(_)) => res.push(connector_op!(
                RemoteFsConnectorOp::Copy,
                format!("Modify remote file at {}/{}", addr.hostname, addr.path.to_string_lossy())
            )),
            (None, Some(_)) => {
                //RemoteFs push
                res.push(connector_op!(
                    RemoteFsConnectorOp::Copy,
                    format!("Create new remote file at {}/{}", addr.hostname, addr.path.to_string_lossy())
                ));
            }
        }

        for hook in post_hooks {
            res.push(connector_op!(
                RemoteFsConnectorOp::Exec(hook.clone()),
                format!("Execute hook: {}", hook.shell)
            ));
        }

        Ok(res)
    }

    async fn op_exec(&self, addr: &Path, op: &str) -> Result<OpExecResponse, anyhow::Error> {
        let op = RemoteFsConnectorOp::from_str(op)?;
        let addr = RemoteFsPath::from_path(addr)?;

        let config = self.config.lock().await;

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
                // self.client.copy(&addr.path, &remote_path)?;
                let client = self.get_client(&addr.hostname).await?;
                let client = &mut *client.lock().await;
                // println!("COPY: pwd = {:?}", client.pwd()?);

                let Some(host) = config.hosts.get(&addr.hostname) else {
                    bail!("Host {} not in config", addr.hostname);
                };
                let mounts = &host.mounts;

                // We reverse the mount list to pick the last mount that matches the globs, on the
                // assumption that partially redundant mounts are listed in order of most general -> most specific.
                for mount in mounts.iter().rev() {
                    // test if we match file paths directly
                    if mount.path_matches_mount(&remote_path) {
                        // yeah, right here is fine, thanks
                        let size = std::fs::metadata(&local_path)?.len();
                        let metadata = Metadata {
                            accessed: None,
                            created: None,
                            modified: None,
                            uid: mount.uid,
                            gid: mount.gid,
                            mode: mount.mode.map(UnixPex::from),
                            size,
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
            }
            RemoteFsConnectorOp::Delete => {
                let remote_path = PathBuf::from("/").join(&addr.path);
                let client = self.get_client(&addr.hostname).await?;
                let client = &mut *client.lock().await;

                client.remove_file(&remote_path)?;

                return op_exec_output!(format!(
                    "Deleted remote file at {}/{}",
                    addr.hostname,
                    addr.path.to_string_lossy()
                ));
            }
            RemoteFsConnectorOp::Exec(hook) => {
                let client = self.get_client(&addr.hostname).await?;
                let client = &mut *client.lock().await;

                if let Some(work_dir) = hook.work_dir {
                    let old_workdir = client.pwd()?;
                    client.change_dir(&work_dir)?;
                    client.exec(&hook.shell)?;
                    client.change_dir(&old_workdir)?;
                } else {
                    client.exec(&hook.shell)?;
                }

                return op_exec_output!(format!("Executed hook"));
            }
        }
    }

    async fn eq(&self, addr: &Path, a: &[u8], b: &[u8]) -> Result<bool, anyhow::Error> {
        Ok(a == b)
    }

    async fn diag(&self, addr: &Path, a: &[u8]) -> Result<Option<DiagnosticResponse>, anyhow::Error> {
        if addr == PathBuf::from("remotefs/config.ron") {
            ron_check_syntax::<RemoteFsHost>(a)
        } else {
            Ok(None)
        }
    }
}
