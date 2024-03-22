use crate::msg::VaultAsset;
use cosmwasm_schema::cw_serde;
use cosmwasm_std::Empty;
use cosmwasm_std::{Addr, Coin, Decimal, Uint128};
use cw_storage_plus::{Item, Map};

/// Top level storage key. Values must not conflict.
/// Each key is only one byte long to ensure we use the smallest possible storage keys.
#[repr(u8)]
pub enum TopKey {
    Owner = b'a',
    Operator = b'b',
    Config = b'c',
    PoolId = b'd',
    VaultAssets = b'e',
    CommissionRate = b'f',
    CommissionRewards = b'g',
    UncompoundedRewards = b'h',
    WhitelistedDepositors = b'i',
    VaultDenom = b'j',
    Supply = b'k',
    LastUpdate = b'l',
    AssetPendingMint = b'm',
    AccountsPendingMint = b'n',
    AccountsPendingBurn = b'o',
    PositionOpen = b'p',
    CapReached = b'q',
    Halted = b'r',
    Terminated = b's',
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

// Contract Owner
pub const OWNER: Item<Addr> = Item::new(TopKey::Owner.as_str());
// Contract Operator
pub const OPERATOR: Item<Addr> = Item::new(TopKey::Operator.as_str());
// Contract Config
pub const CONFIG: Item<Config> = Item::new(TopKey::Config.as_str());
// CL pool id
pub const POOL_ID: Item<u64> = Item::new(TopKey::PoolId.as_str());
// Assets that can be deposited in the vault
pub const VAULT_ASSETS: Item<(VaultAsset, VaultAsset)> = Item::new(TopKey::VaultAssets.as_str());
// Tokenfactory denom for the vault token
// rate to charge for the vault
pub const COMMISSION_RATE: Item<Decimal> = Item::new(TopKey::CommissionRate.as_str());
// collected commissions
pub const COMMISSION_REWARDS: Item<Vec<Coin>> = Item::new(TopKey::CommissionRewards.as_str());
// collected rewards that are not asset0 or asset1
pub const UNCOMPOUNDED_REWARDS: Item<Vec<Coin>> = Item::new(TopKey::UncompoundedRewards.as_str());
pub const WHITELISTED_DEPOSITORS: Map<Addr, Empty> =
    Map::new(TopKey::WhitelistedDepositors.as_str());
pub const VAULT_DENOM: Item<String> = Item::new(TopKey::VaultDenom.as_str());
// Total supply of vault tokens
pub const SUPPLY: Item<Uint128> = Item::new(TopKey::Supply.as_str());
// Last time exits and joins were processed
pub const LAST_UPDATE: Item<u64> = Item::new(TopKey::LastUpdate.as_str());
// Assets waiting to join the vault
pub const ASSETS_PENDING_MINT: Item<Vec<Coin>> = Item::new(TopKey::AssetPendingMint.as_str());
// Accounts pending activation and how much for each one
pub const ACCOUNTS_PENDING_MINT: Map<Addr, (Vec<Coin>, Uint128)> =
    Map::new(TopKey::AccountsPendingMint.as_str());
// Addresses pending to leave the vault
pub const ACCOUNTS_PENDING_BURN: Map<Addr, Uint128> =
    Map::new(TopKey::AccountsPendingBurn.as_str());
// Flag to indicate if the vault has an active position
pub const POSITION_OPEN: Item<bool> = Item::new(TopKey::PositionOpen.as_str());
// Flag to indicate if the vault cap has been reached and no more people can join (they can leave though)
pub const CAP_REACHED: Item<bool> = Item::new(TopKey::CapReached.as_str());
// Flag to halt joins and exits (in case of some emergency)
pub const HALTED: Item<bool> = Item::new(TopKey::Halted.as_str());
// Flag to indicate that the vault has been terminated by owner
pub const TERMINATED: Item<bool> = Item::new(TopKey::Terminated.as_str());

#[cw_serde]
pub struct Config {
    pub metadata: Option<Metadata>,
    pub min_asset0: Uint128,
    pub min_asset1: Uint128,
    pub min_redemption: Option<Uint128>,
    pub dollar_cap: Option<Uint128>,
    pub pyth_contract_address: Addr,
    pub price_expiry: u64,
    pub commission_receiver: Addr,
}

#[cw_serde]
pub struct Metadata {
    pub name: String,
    pub description: Option<String>,
    pub image: Option<String>,
}
