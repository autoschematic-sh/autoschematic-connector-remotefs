use autoschematic_core::connector::{Resource, ResourceAddress};
use serde::{Deserialize, Serialize};


#[derive(Debug, Serialize, Deserialize)]
pub struct FileContents {
    pub contents: String,
}

impl Resource for FileContents {
    fn to_string(&self) -> Result<String, anyhow::Error> {
        Ok(self.contents.clone())
    }

    fn from_str(addr: &impl ResourceAddress, s: &str) -> Result<Self, anyhow::Error>
    where
        Self: Sized,
    {
        Ok(FileContents {
            contents: s.to_string(),
        })
    }
}
