use autoschematic_core::connector::{Resource, ResourceAddress};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct FileContents {
    pub contents: Vec<u8>,
}

impl Resource for FileContents {
    fn to_bytes(&self) -> Result<Vec<u8>, anyhow::Error> {
        Ok(self.contents.clone())
    }

    fn from_bytes(addr: &impl ResourceAddress, s: &[u8]) -> Result<Self, anyhow::Error>
    where
        Self: Sized,
    {
        Ok(FileContents { contents: s.to_vec() })
    }
}
