use macroquad::logging as log;
use std::sync::Arc;

pub trait Storage {
    fn get(&self, key: &str) -> Result<Option<String>, anyhow::Error>;
    fn set(&self, key: &str, value: String) -> Result<(), anyhow::Error>;
}

pub struct FilesystemStorage {
    db: sled::Db,
}

impl Storage for FilesystemStorage {
    fn get(&self, key: &str) -> Result<Option<String>, anyhow::Error> {
        log::debug!("Storage: getting {}", key);
        match self.db.get(key)? {
            Some(value) => Ok(Some(String::from_utf8(value.to_vec())?)),
            None => Ok(None),
        }
    }

    fn set(&self, key: &str, value: String) -> Result<(), anyhow::Error> {
        log::debug!("Storage: setting {}", key);

        self.db.insert(key, value.into_bytes())?;
        Ok(())
    }
}

pub type DefaultStorage = FilesystemStorage;

pub fn init_storage() -> Result<Arc<DefaultStorage>, anyhow::Error> {
    let dir = match dirs::state_dir().or_else(|| dirs::config_dir()) {
        Some(parent) => parent.join("nertsio/storage"),
        None => std::path::PathBuf::from("nertsio_storage"),
    };

    let db = sled::open(dir)?;

    Ok(Arc::new(FilesystemStorage { db }))
}
