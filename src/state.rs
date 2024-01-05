use cosmwasm_schema::cw_serde;
use cosmwasm_std::{Addr, Coin, Decimal};
use cw_storage_plus::{Item, Map};

use crate::msg::{Frequency, PythAsset};

/// Top level storage key. Values must not conflict.
/// Each key is only one byte long to ensure we use the smallest possible storage keys.
#[repr(u8)]
pub enum TopKey {
    Config = b'd',
    VaultRatio = b'e',
    LastUpdate = b'f',
    AssetPendingActivation = b'g',
    AccountsPendingActivation = b'h',
    CurrentPositions = b'i',
    AddressesWaitingForExit = b'j',
    HaltExitsAndJoins = b'k',
    CapReached = b'l',
}

impl TopKey {
    const fn as_str(&self) -> &str {
        let array_ref = unsafe { std::mem::transmute::<_, &[u8; 1]>(self) };
        match core::str::from_utf8(array_ref) {
            Ok(a) => a,
            Err(_) => panic!("Non-utf8 enum value found. Use a-z, A-Z and 0-9"),
        }
    }
}

// Contract Config
pub const CONFIG: Item<Config> = Item::new(TopKey::Config.as_str());
// Vault ratio that each address owns
pub const VAULT_RATIO: Map<Addr, Decimal> = Map::new(TopKey::VaultRatio.as_str());
// Last time the vault was updated
pub const LAST_UPDATE: Item<u64> = Item::new(TopKey::LastUpdate.as_str());
// Assets waiting to join the vault
pub const ASSETS_PENDING_ACTIVATION: Item<Vec<Coin>> =
    Item::new(TopKey::AssetPendingActivation.as_str());
// Accounts pending activation and how much for each one
pub const ACCOUNTS_PENDING_ACTIVATION: Map<Addr, Vec<Coin>> =
    Map::new(TopKey::AccountsPendingActivation.as_str());
// Addresses pending to leave the vault
pub const ADDRESSES_WAITING_FOR_EXIT: Item<Vec<Addr>> =
    Item::new(TopKey::AddressesWaitingForExit.as_str());
// Flag to halt joins and exits (in case of some emergency)
pub const HALT_EXITS_AND_JOINS: Item<bool> = Item::new(TopKey::HaltExitsAndJoins.as_str());
// Flag to indicate if the vault cap has been reached and no more people can join (they can leave though)
pub const CAP_REACHED: Item<bool> = Item::new(TopKey::CapReached.as_str());

#[cw_serde]
pub struct Config {
    pub pool_id: u64,
    pub asset1: PythAsset,
    pub asset2: PythAsset,
    pub dollar_cap: Option<u32>,
    pub pyth_contract_address: Addr,
    pub update_frequency: Frequency,
    pub exit_commission: Option<Decimal>,
    pub commission_receiver: Addr,
}
