use autoschematic_core::tarpc_bridge::tarpc_connector_main;
use connector::RemoteFsConnector;

pub mod connector;
pub mod config;
pub mod addr;
pub mod resource;


#[tokio::main]
pub async fn main() -> anyhow::Result<()> {
    tarpc_connector_main::<RemoteFsConnector>().await?;
    Ok(())
}
