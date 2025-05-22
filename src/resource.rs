use std::ffi::{OsStr, OsString};

use autoschematic_core::connector::{Resource, ResourceAddress};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct FileContents {
    pub contents: OsString,
}

impl Resource for FileContents {
    fn to_os_string(&self) -> Result<OsString, anyhow::Error> {
        Ok(self.contents.clone())
    }

    fn from_os_str(addr: &impl ResourceAddress, s: &OsStr) -> Result<Self, anyhow::Error>
    where
        Self: Sized,
    {
        Ok(FileContents {
            contents: s.to_os_string(),
        })
    }
}
