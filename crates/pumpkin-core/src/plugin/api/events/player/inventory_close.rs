use std::sync::Arc;

use crate::entity::player::Player;
use pumpkin_data::screen::WindowType;
use pumpkin_macros::Event;

use super::PlayerEvent;

/// Event that is triggered when a player closes an inventory.
#[derive(Event, Clone)]
pub struct InventoryCloseEvent {
    /// The player who closed the inventory.
    pub player: Arc<Player>,

    /// The window type of the inventory that was closed.
    pub window_type: Option<WindowType>,

    /// Protocol screen handler ID, matching the GUI open result.
    pub window_id: u32,
}

impl InventoryCloseEvent {
    /// Creates a new instance of `InventoryCloseEvent`.
    ///
    /// # Arguments
    ///
    /// - `player`: A reference-counted pointer to the player who triggered the event.
    /// - `window_type`: The window type of the inventory.
    ///
    /// # Returns
    ///
    /// A new `InventoryCloseEvent` instance with the specified data.
    pub fn new(player: &Arc<Player>, window_type: Option<WindowType>, window_id: u32) -> Self {
        Self {
            player: Arc::clone(player),
            window_type,
            window_id,
        }
    }
}

impl PlayerEvent for InventoryCloseEvent {
    fn get_player(&self) -> &Arc<Player> {
        &self.player
    }
}
