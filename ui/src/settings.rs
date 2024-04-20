#[cfg(target_family = "wasm")]
use futures_util::StreamExt;
use macroquad::logging as log;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::sync::{Arc, Mutex};

fn default_name() -> String {
    "Nerter".to_owned()
}

#[derive(Clone, PartialEq, Deserialize, Serialize, Default)]
pub enum DragMode {
    #[default]
    Click,
    Drag,
    Hybrid,
}

#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub enum CardTheme {
    Standard,
    HighVisibility,
}

impl Default for CardTheme {
    fn default() -> Self {
        CardTheme::Standard
    }
}

#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct Settings {
    #[serde(default = "default_name")]
    pub name: String,

    #[serde(default)]
    pub drag_mode: DragMode,

    #[serde(default)]
    pub card_theme: CardTheme,

    #[serde(default)]
    pub round_start_music: bool,

    #[serde(default)]
    pub suit_callouts: bool,

    #[serde(default)]
    pub nerts_callout: bool,

    #[serde(rename = "drag")]
    #[serde(skip_serializing)]
    pub legacy_drag: Option<bool>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            name: default_name(),
            drag_mode: Default::default(),
            card_theme: Default::default(),
            round_start_music: false,
            suit_callouts: false,
            nerts_callout: false,
            legacy_drag: None,
        }
    }
}

#[cfg(target_family = "wasm")]
const SETTINGS_KEY: &str = "nertsioSettings";

#[cfg(target_family = "wasm")]
async fn run_settings_save_loop(
    storage: web_sys::Storage,
    init_value: Settings,
    mutex: Arc<Mutex<Settings>>,
) {
    log::debug!("run_settings_save_loop");

    let mut saved_value = init_value;

    let mut interval = futures_ticker::Ticker::new(std::time::Duration::from_secs(5));

    loop {
        interval.next().await;

        if {
            let lock = mutex.lock().unwrap();
            if saved_value != *lock {
                saved_value = (*lock).clone();
                true
            } else {
                false
            }
        } {
            // value changed, need to save it

            if let Err(err) = async {
                let buf = serde_json::to_string(&saved_value)?;

                storage
                    .set_item(SETTINGS_KEY, &buf)
                    .map_err(|err| anyhow::anyhow!("Failed to set item: {:?}", err))
            }
            .await
            {
                log::error!("failed to save settings: {:?}", err);
            }
        }
    }
}

#[cfg(not(target_family = "wasm"))]
async fn run_settings_save_loop(
    config_path: std::path::PathBuf,
    init_value: Settings,
    mutex: Arc<Mutex<Settings>>,
) {
    let mut saved_value = init_value;

    let file = Arc::new(atomicwrites::AtomicFile::new(
        config_path,
        atomicwrites::OverwriteBehavior::AllowOverwrite,
    ));

    let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        interval.tick().await;

        if {
            let lock = mutex.lock().unwrap();
            if saved_value != *lock {
                saved_value = (*lock).clone();
                true
            } else {
                false
            }
        } {
            // value changed, need to save it

            if let Err(err) = async {
                let buf = serde_json::to_vec(&saved_value)?;
                let file = file.clone();
                tokio::task::spawn_blocking(move || {
                    file.write(|f| {
                        use std::io::Write;

                        f.write_all(&buf)
                    })?;

                    Result::<_, anyhow::Error>::Ok(())
                })
                .await?
            }
            .await
            {
                log::error!("failed to save settings: {:?}", err);
            }
        }
    }
}

pub fn init_settings(async_rt: &crate::AsyncRt) -> Arc<Mutex<Settings>> {
    let settings_mutex;
    #[cfg(not(target_family = "wasm"))]
    {
        #[cfg(target_os = "android")]
        let config_dir = {
            // https://stackoverflow.com/a/6284443/2533397

            let cmdline = std::fs::read_to_string("/proc/self/cmdline").unwrap();

            // seems to be full of nul bytes, stop at the first one
            let cmdline = match cmdline.find('\0') {
                None => &cmdline,
                Some(idx) => &cmdline[..idx],
            };

            let mut result = std::path::PathBuf::from("/data/data/");
            result.push(std::path::Path::new(cmdline));

            result
        };
        #[cfg(not(target_os = "android"))]
        let config_dir = dirs::config_dir()
            .map(Cow::Owned)
            .unwrap_or_else(|| std::path::Path::new(".").into());

        let config_path = config_dir.join("nertsio.json");

        log::debug!("config path: {:?}", config_path);

        match std::fs::File::open(&config_path) {
            Ok(mut file) => {
                let mut init_value: Settings = match serde_json::from_reader(&mut file) {
                    Ok(value) => value,
                    Err(err) => {
                        log::debug!("Failed to parse config file: {:?}", err);
                        log::debug!("Will reset config to defaults.");

                        Default::default()
                    }
                };

                if let Some(drag) = init_value.legacy_drag {
                    init_value.drag_mode = if drag {
                        DragMode::Hybrid
                    } else {
                        DragMode::Click
                    };
                }

                settings_mutex = Arc::new(Mutex::new(init_value.clone()));
                async_rt.spawn(run_settings_save_loop(
                    config_path,
                    init_value,
                    settings_mutex.clone(),
                ));
            }
            Err(err) => {
                if err.kind() == std::io::ErrorKind::NotFound {
                    log::debug!("config file not found, creating new");

                    let init_value: Settings = Default::default();

                    settings_mutex = Arc::new(Mutex::new(init_value.clone()));
                    async_rt.spawn(run_settings_save_loop(
                        config_path,
                        init_value,
                        settings_mutex.clone(),
                    ));
                } else {
                    log::error!("Failed to open settings file: {:?}", err);
                    log::error!("Settings will not be saved.");

                    settings_mutex = Arc::new(Mutex::new(Default::default()));
                }
            }
        }
    }
    #[cfg(target_family = "wasm")]
    {
        match web_sys::window()
            .ok_or_else(|| anyhow::anyhow!("Can't access window"))
            .and_then(|window| {
                window
                    .local_storage()
                    .map_err(|err| anyhow::anyhow!("Can't access localStorage: {:?}", err))
                    .and_then(|x| x.ok_or_else(|| anyhow::anyhow!("Can't access localStorage")))
            }) {
            Ok(storage) => {
                let init_value: Settings = match storage.get_item(SETTINGS_KEY) {
                    Ok(None) => Default::default(),
                    Ok(Some(buf)) => match serde_json::from_str(&buf) {
                        Ok(value) => value,
                        Err(err) => {
                            log::debug!("Failed to parse config file: {:?}", err);
                            log::debug!("Will reset config to defaults.");

                            Default::default()
                        }
                    },
                    Err(err) => {
                        log::debug!("Failed to fetch config file: {:?}", err);
                        log::debug!("Will reset config to defaults.");

                        Default::default()
                    }
                };

                settings_mutex = Arc::new(Mutex::new(init_value.clone()));
                async_rt.spawn(run_settings_save_loop(
                    storage,
                    init_value,
                    settings_mutex.clone(),
                ));
            }
            Err(err) => {
                log::error!("Failed to init settings: {:?}", err);
                log::error!("Settings will not be saved.");

                settings_mutex = Arc::new(Mutex::new(Default::default()));
            }
        }
    }

    settings_mutex
}
