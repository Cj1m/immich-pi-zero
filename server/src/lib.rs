//! Pi Immich server. Responses match Immich v3.3.6 where the official mobile app or web UI can see them.

mod albums;
mod config;
mod db;
mod exif_meta;
mod http;
mod http_config;
mod http_media;
mod http_memories;
mod http_search;
mod http_shares;
mod media;
mod memories;
mod ml;
mod shares;
mod store;
mod sync;
mod system_config;
mod thumbs;
mod web;

use std::net::SocketAddr;

pub use config::{Config, StartupError};
pub use db::App;
#[doc(hidden)]
pub use exif_meta::{embed_exif, ExifFixture};
pub use http::router;

/// Open the database, create the first admin when the server has no User, and serve HTTP.
pub fn run() -> Result<(), StartupError> {
    let config = Config::from_env()?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| StartupError::Io(std::io::Error::other(error)))?;
    runtime.block_on(async move {
        let app = App::prepare(&config).await?;
        let jobs = app.clone();
        tokio::spawn(async move {
            memories::nightly_loop(jobs).await;
        });
        let address = SocketAddr::new(config.host, config.port);
        let listener = tokio::net::TcpListener::bind(address)
            .await
            .map_err(StartupError::Io)?;
        eprintln!("immich listening on {address}");
        axum::serve(listener, http::router(app))
            .await
            .map_err(StartupError::Io)?;
        Ok(())
    })
}
