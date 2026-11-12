//! Typed view of the `purpur.yml` settings `RustGot` applies.
//!
//! Every value is read like Purpur's `PurpurConfig`/`PurpurWorldConfig` getters: the admin's value
//! when it has the right type, otherwise Purpur's default from the built-in file.

use pumpkin_util::text::TextComponent;
use serde_norway::Value;
use tracing::{error, warn};

use crate::{
    minimessage,
    tree::{self, yaml11_bool},
};

/// Global settings (the `settings` section).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalSettings {
    /// `settings.server-mod-name`. `None` while it is Purpur's default, which stands for the
    /// server's own brand.
    pub server_mod_name: Option<String>,
    /// `settings.startup-commands`, run as console commands once the server is up.
    pub startup_commands: Vec<String>,
    /// `settings.messages`.
    pub messages: Messages,
}

/// `settings.messages`, in `MiniMessage` format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Messages {
    /// Broadcast when a player goes AFK; `%s` is the player name.
    pub afk_broadcast_away: String,
    /// Broadcast when an AFK player is back; `%s` is the player name.
    pub afk_broadcast_back: String,
}

impl Messages {
    /// The AFK broadcast for `player_name` going AFK (`afk`) or coming back, or `None` when the
    /// message is empty.
    #[must_use]
    pub fn afk_broadcast(&self, afk: bool, player_name: &str) -> Option<TextComponent> {
        let message = if afk {
            &self.afk_broadcast_away
        } else {
            &self.afk_broadcast_back
        };
        (!message.is_empty()).then(|| minimessage::parse(&java_format(message, player_name)))
    }
}

/// Java's `String.format(message, argument)` for the `%s`, `%%` and `%n` conversions.
fn java_format(message: &str, argument: &str) -> String {
    let mut formatted = String::with_capacity(message.len() + argument.len());
    let mut chars = message.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            formatted.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => formatted.push_str(argument),
            Some('n') => formatted.push('\n'),
            Some(other) => {
                formatted.push('%');
                if other != '%' {
                    formatted.push(other);
                }
            }
            None => formatted.push('%'),
        }
    }
    formatted
}

/// Settings of one world, resolved from `world-settings.<world>` over `world-settings.default`.
#[derive(Debug, Clone, PartialEq)]
pub struct WorldSettings {
    /// `blocks`.
    pub blocks: Blocks,
    /// `gameplay-mechanics`.
    pub gameplay_mechanics: GameplayMechanics,
    /// `hunger`.
    pub hunger: Hunger,
}

/// `blocks` (world).
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(clippy::struct_excessive_bools, reason = "mirrors purpur.yml")]
pub struct Blocks {
    /// `blocks.cactus.breaks-from-solid-neighbors`.
    pub cactus_breaks_from_solid_neighbors: bool,
    /// `blocks.campfire.lit-when-placed`.
    pub campfire_lit_when_placed: bool,
    /// `blocks.coral.die-outside-water`.
    pub coral_die_outside_water: bool,
    /// `blocks.dragon_egg.teleport`.
    pub dragon_egg_teleport: bool,
    /// `blocks.farmland.disable-trampling`.
    pub farmland_disable_trampling: bool,
    /// `blocks.farmland.only-players-trample`.
    pub farmland_only_players_trample: bool,
}

/// `gameplay-mechanics` (world).
#[derive(Debug, Clone, PartialEq)]
pub struct GameplayMechanics {
    /// `gameplay-mechanics.disable-drops-on-cramming-death`: no loot-table drops for entities
    /// killed by entity cramming.
    pub disable_drops_on_cramming_death: bool,
    /// `gameplay-mechanics.mob-effects`.
    pub mob_effects: MobEffects,
    /// `gameplay-mechanics.player`.
    pub player: PlayerMechanics,
}

/// `gameplay-mechanics.mob-effects`.
#[derive(Debug, Clone, PartialEq)]
pub struct MobEffects {
    /// Health restored per Regeneration tick.
    pub health_regen_amount: f32,
    /// Poison only hurts while health is above this.
    pub minimal_health_poison_amount: f32,
    /// Damage per Poison tick.
    pub poison_degeneration_amount: f32,
    /// Damage per Wither tick.
    pub wither_degeneration_amount: f32,
    /// Exhaustion per tick and level of Hunger.
    pub hunger_exhaustion_amount: f32,
    /// Saturation modifier of the Saturation effect.
    pub saturation_regen_amount: f32,
}

/// `gameplay-mechanics.player`.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerMechanics {
    /// Damage multiplier of critical hits.
    pub critical_damage_multiplier: f64,
    /// `gameplay-mechanics.player.idle-timeout.kick-if-idle`: when false, players past the idle
    /// timeout are marked AFK instead of kicked.
    pub idle_timeout_kick_if_idle: bool,
}

/// `hunger` (world).
#[derive(Debug, Clone, PartialEq)]
pub struct Hunger {
    /// Damage per starvation hit.
    pub starvation_damage: f32,
}

/// Reads typed values with Bukkit's fallback rules and records which paths `RustGot` supports.
pub struct Reader<'a> {
    /// Admin sections, most specific first, each with the prefix used in warnings (`None` to
    /// not warn about it).
    sources: Vec<(Option<String>, &'a Value)>,
    builtin: &'a Value,
    /// Paths read so far, relative to the section root.
    pub supported: Vec<&'static str>,
    /// Paths missing from (or mistyped in) the built-in defaults; only a bug can cause these.
    pub missing_defaults: Vec<&'static str>,
}

impl<'a> Reader<'a> {
    /// `sources` are tried in order before falling back to `builtin`.
    pub const fn new(sources: Vec<(Option<String>, &'a Value)>, builtin: &'a Value) -> Self {
        Self {
            sources,
            builtin,
            supported: Vec::new(),
            missing_defaults: Vec::new(),
        }
    }

    fn read<T>(
        &mut self,
        path: &'static str,
        kind: &str,
        convert: impl Fn(&Value) -> Option<T>,
    ) -> Option<T> {
        self.supported.push(path);
        for (prefix, source) in &self.sources {
            if let Some(value) = tree::get(source, path) {
                if let Some(value) = convert(value) {
                    return Some(value);
                }
                if let Some(prefix) = prefix {
                    warn!("purpur.yml: {prefix}{path} must be {kind}; ignoring it");
                }
            }
        }
        let value = tree::get(self.builtin, path).and_then(convert);
        if value.is_none() {
            error!("Built-in Purpur defaults have no {kind} at {path}");
            self.missing_defaults.push(path);
        }
        value
    }

    /// Bukkit `getBoolean`.
    pub fn bool(&mut self, path: &'static str) -> bool {
        self.read(path, "true or false", |value| match value {
            Value::Bool(flag) => Some(*flag),
            Value::String(text) => yaml11_bool(text),
            _ => None,
        })
        .unwrap_or_default()
    }

    /// Bukkit `getDouble`.
    pub fn f64(&mut self, path: &'static str) -> f64 {
        self.read(path, "a number", Value::as_f64)
            .unwrap_or_default()
    }

    /// Bukkit `getDouble` narrowed to `float`, as Purpur does for float fields.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Purpur casts these doubles to float"
    )]
    pub fn f32(&mut self, path: &'static str) -> f32 {
        self.f64(path) as f32
    }

    /// Bukkit `getString`: any scalar as text.
    pub fn string(&mut self, path: &'static str) -> String {
        self.read(path, "text", tree::scalar_string)
            .unwrap_or_default()
    }

    /// Bukkit `getList` with every entry as text.
    pub fn strings(&mut self, path: &'static str) -> Vec<String> {
        self.read(path, "a list", |value| {
            value
                .as_sequence()
                .map(|list| list.iter().filter_map(tree::scalar_string).collect())
        })
        .unwrap_or_default()
    }
}

/// Purpur's default `server-mod-name`, which is its own brand.
const PURPUR_BRAND: &str = "Purpur";

impl GlobalSettings {
    pub(crate) fn read(reader: &mut Reader<'_>) -> Self {
        let server_mod_name = reader.string("settings.server-mod-name");
        Self {
            server_mod_name: (server_mod_name != PURPUR_BRAND).then_some(server_mod_name),
            startup_commands: reader
                .strings("settings.startup-commands")
                .into_iter()
                .map(|mut command| {
                    if command.starts_with('/') {
                        command.remove(0);
                    }
                    command
                })
                .collect(),
            messages: Messages {
                afk_broadcast_away: reader.string("settings.messages.afk-broadcast-away"),
                afk_broadcast_back: reader.string("settings.messages.afk-broadcast-back"),
            },
        }
    }
}

impl WorldSettings {
    pub(crate) fn read(reader: &mut Reader<'_>) -> Self {
        Self {
            blocks: Blocks {
                cactus_breaks_from_solid_neighbors: reader
                    .bool("blocks.cactus.breaks-from-solid-neighbors"),
                campfire_lit_when_placed: reader.bool("blocks.campfire.lit-when-placed"),
                coral_die_outside_water: reader.bool("blocks.coral.die-outside-water"),
                dragon_egg_teleport: reader.bool("blocks.dragon_egg.teleport"),
                farmland_disable_trampling: reader.bool("blocks.farmland.disable-trampling"),
                farmland_only_players_trample: reader.bool("blocks.farmland.only-players-trample"),
            },
            gameplay_mechanics: GameplayMechanics {
                disable_drops_on_cramming_death: reader
                    .bool("gameplay-mechanics.disable-drops-on-cramming-death"),
                mob_effects: MobEffects {
                    health_regen_amount: reader
                        .f32("gameplay-mechanics.mob-effects.health-regen-amount"),
                    minimal_health_poison_amount: reader
                        .f32("gameplay-mechanics.mob-effects.minimal-health-poison-amount"),
                    poison_degeneration_amount: reader
                        .f32("gameplay-mechanics.mob-effects.poison-degeneration-amount"),
                    wither_degeneration_amount: reader
                        .f32("gameplay-mechanics.mob-effects.wither-degeneration-amount"),
                    hunger_exhaustion_amount: reader
                        .f32("gameplay-mechanics.mob-effects.hunger-exhaustion-amount"),
                    saturation_regen_amount: reader
                        .f32("gameplay-mechanics.mob-effects.saturation-regen-amount"),
                },
                player: PlayerMechanics {
                    critical_damage_multiplier: reader
                        .f64("gameplay-mechanics.player.critical-damage-multiplier"),
                    idle_timeout_kick_if_idle: reader
                        .bool("gameplay-mechanics.player.idle-timeout.kick-if-idle"),
                },
            },
            hunger: Hunger {
                starvation_damage: reader.f32("hunger.starvation-damage"),
            },
        }
    }
}
