use crate::entity::player::Player;
use base64::{Engine as _, engine::general_purpose};
use core::error;
use pumpkin_config::BasicConfiguration;
use pumpkin_data::packet::{CURRENT_MC_VERSION, LOWEST_SUPPORTED_MC_VERSION};
use pumpkin_protocol::{
    Players, Sample, StatusResponse, Version,
    codec::var_int::VarInt,
    java::client::{config::CPluginMessage, status::CStatusResponse},
};
use pumpkin_util::text::TextComponent;
use std::{fs, path::Path};
use tracing::{debug, info, warn};
use uuid::Uuid;

const DEFAULT_ICON: &[u8] = include_bytes!("../../../../assets/default_icon.png");
const MAX_SAMPLE_PLAYERS: usize = 12;

fn load_icon_from_file<P: AsRef<Path>>(path: P) -> Result<String, Box<dyn error::Error>> {
    let buf = fs::read(path)?;
    if buf.len() >= 24 {
        let width = u32::from_be_bytes([buf[16], buf[17], buf[18], buf[19]]);
        let height = u32::from_be_bytes([buf[20], buf[21], buf[22], buf[23]]);

        if width != 64 || height != 64 {
            return Err("Invalid favicon dimensions (must be 64x64)".into());
        }
    }
    Ok(load_icon_from_bytes(&buf))
}

fn load_icon_from_bytes(png_data: &[u8]) -> String {
    assert!(!png_data.is_empty(), "PNG data is empty");
    let mut result = "data:image/png;base64,".to_owned();
    general_purpose::STANDARD.encode_string(png_data, &mut result);
    result
}

pub struct CachedStatus {
    pub status_response: StatusResponse,
    player_samples: Vec<(Uuid, String)>,
}

pub struct CachedBranding {
    /// Cached server brand buffer so we don't have to rebuild them every time a player joins
    cached_server_brand: Vec<u8>,
}

impl Default for CachedBranding {
    fn default() -> Self {
        Self::new()
    }
}

impl CachedBranding {
    const BRAND: &'static str = "RustGot";

    #[must_use]
    pub fn new() -> Self {
        let brand = rustgot_purpur::get()
            .settings
            .server_mod_name
            .as_deref()
            .unwrap_or(Self::BRAND);
        // The payload is a protocol string: VarInt length, then the UTF-8 bytes.
        let mut bytes = Vec::with_capacity(VarInt::MAX_SIZE.get() + brand.len());
        let _ = VarInt(i32::try_from(brand.len()).unwrap_or(i32::MAX)).encode(&mut bytes);
        bytes.extend_from_slice(brand.as_bytes());
        Self {
            cached_server_brand: bytes,
        }
    }

    #[must_use]
    pub const fn get_branding(&self) -> CPluginMessage<'_> {
        CPluginMessage::new("minecraft:brand", self.cached_server_brand.as_slice())
    }
}

impl CachedStatus {
    #[must_use]
    pub fn new(config: &BasicConfiguration, motd: &str, max_players: u32) -> Self {
        let status_response = Self::build_response(config, motd, max_players);

        Self {
            status_response,
            player_samples: Vec::new(),
        }
    }

    pub fn get_status_response(&self, client_protocol: i32) -> StatusResponse {
        let mut response = self.status_response.clone();

        let supported_min = LOWEST_SUPPORTED_MC_VERSION.protocol_version();
        let supported_max = CURRENT_MC_VERSION.protocol_version();

        if client_protocol >= supported_min
            && client_protocol <= supported_max
            && let Some(version) = &mut response.version
        {
            version.protocol = client_protocol as u32;
        }

        response
    }

    pub fn get_status_packet(&self, client_protocol: i32) -> CStatusResponse {
        let response = self.get_status_response(client_protocol);
        let json = serde_json::to_string(&response).unwrap_or_default();
        CStatusResponse::new(json)
    }

    fn build_sample_list(&self) -> Vec<Sample> {
        self.player_samples
            .iter()
            .take(MAX_SAMPLE_PLAYERS)
            .map(|(id, name)| Sample {
                name: name.clone(),
                id: id.to_string(),
            })
            .collect()
    }

    pub fn add_player(&mut self, player: &Player) {
        let player_id = player.gameprofile.id;
        let player_name = player.gameprofile.name.clone();

        if !self.player_samples.iter().any(|(id, _)| *id == player_id) {
            self.player_samples.push((player_id, player_name));
            let sample = self.build_sample_list();

            if let Some(players) = &mut self.status_response.players {
                players.online = players.online.saturating_add(1);
                players.sample = sample;
            }
        }
    }

    pub fn remove_player(&mut self, player: &Player) {
        let player_id = player.gameprofile.id;

        if self.player_samples.iter().any(|(id, _)| *id == player_id) {
            self.player_samples.retain(|(id, _)| *id != player_id);
            let sample = self.build_sample_list();

            if let Some(players) = &mut self.status_response.players {
                players.online = players.online.saturating_sub(1);
                players.sample = sample;
            }
        }
    }

    pub fn build_response(
        config: &BasicConfiguration,
        motd: &str,
        max_players: u32,
    ) -> StatusResponse {
        let favicon = if config.use_favicon {
            config.favicon_path.as_ref().map_or_else(
                || {
                    debug!("Loading default icon");

                    // Attempt to load default icon
                    Some(load_icon_from_bytes(DEFAULT_ICON))
                },
                |icon_path| {
                    if !std::path::Path::new(icon_path)
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
                    {
                        warn!("Favicon is not a PNG-image, using default.");
                        return Some(load_icon_from_bytes(DEFAULT_ICON));
                    }
                    debug!("Attempting to load server favicon from '{icon_path}'");

                    match load_icon_from_file(icon_path) {
                        Ok(icon) => Some(icon),
                        Err(e) => {
                            let error_message = e.downcast_ref::<std::io::Error>().map_or_else(
                                || format!("other error: {e}; using default."),
                                |io_err| {
                                    if io_err.kind() == std::io::ErrorKind::NotFound {
                                        "not found; using default.".to_string()
                                    } else {
                                        format!("I/O error: {io_err}; using default.")
                                    }
                                },
                            );
                            warn!("Failed to load favicon from '{icon_path}': {error_message}");

                            Some(load_icon_from_bytes(DEFAULT_ICON))
                        }
                    }
                },
            )
        } else {
            info!("Favicon usage is disabled.");
            None
        };

        let version = if LOWEST_SUPPORTED_MC_VERSION == CURRENT_MC_VERSION {
            CURRENT_MC_VERSION.to_string()
        } else {
            format!("{LOWEST_SUPPORTED_MC_VERSION}-{CURRENT_MC_VERSION}")
        };
        StatusResponse {
            version: Some(Version {
                // Like Paper's server list ping: `<server-mod-name> <version>`.
                name: match &rustgot_purpur::get().settings.server_mod_name {
                    Some(mod_name) => format!("{mod_name} {version}"),
                    None => version,
                },
                protocol: CURRENT_MC_VERSION.protocol_version() as u32,
            }),
            players: Some(Players {
                max: max_players,
                online: 0,
                sample: vec![],
            }),
            description: TextComponent::text(motd.to_string()),
            favicon,
            // This should stay true even when reports are disabled.
            // It prevents the annoying popup when joining the server.
            enforce_secure_chat: true,
        }
    }
}

impl Default for CachedStatus {
    fn default() -> Self {
        Self::new(
            &BasicConfiguration::default(),
            "A blazingly fast Pumpkin server!",
            1000,
        )
    }
}
