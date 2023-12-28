use cosmwasm_schema::{cw_serde, QueryResponses};
use cosmwasm_std::{Addr, Coin, Decimal};
use cw_ownable::{cw_ownable_execute, cw_ownable_query};
use osmosis_std::types::osmosis::poolmanager::v1beta1::{
    SwapAmountInRoute, SwapAmountInSplitRoute,
};
use pyth_sdk_cw::PriceIdentifier;

use crate::state::Config;

#[cw_serde]
pub struct InstantiateMsg {
    pub pool_id: u64,
    // Update users frequency (adding users that want to join and removing users that want to leave)
    pub update_frequency: Frequency,
    // CL Assets with their corresponding pyth price feed
    pub asset1: PythAsset,
    pub asset2: PythAsset,
    // Exit vault commission (in %)
    pub exit_commission: Option<Decimal>,
    // Flag to take the right pyth contract address - true for mainnet, false for testnet
    pub mainnet: bool,
}

#[cw_serde]
pub enum Frequency {
    Blocks(u64),
    Seconds(u64),
}

#[cw_serde]
pub struct PythAsset {
    pub denom: String,
    pub identifier: PriceIdentifier,
}

#[cw_ownable_execute]
#[cw_serde]
pub enum ExecuteMsg {
    // If for some reason the pyth oracle contract address or the price identifiers change, we can update it (also for testing)
    ModifyConfig {
        config: Config,
    },
    // Create position
    CreatePosition {
        lower_tick: i64,
        upper_tick: i64,
        tokens_provided: Vec<Coin>,
        token_min_amount0: String,
        token_min_amount1: String,
    },
    // Add to position
    AddToPosition {
        position_id: u64,
        amount0: String,
        amount1: String,
        token_min_amount0: String,
        token_min_amount1: String,
    },
    // Withdraw position
    WithdrawPosition {
        position_id: u64,
        liquidity_amount: String,
    },
    // Swap Exact Amount In
    SwapExactAmountIn {
        routes: Vec<SwapAmountInRoute>,
        token_in: Coin,
        token_out_min_amount: String,
    },
    // Split Route Swap Exact Amount In
    SplitRouteSwapExactAmountIn {
        routes: Vec<SwapAmountInSplitRoute>,
        token_in_denom: String,
        token_out_min_amount: String,
    },
    // Process entries and exits (done internally by the contract every update frequency)
    ProcessNewEntriesAndExits {},
    // Join vault
    Join {},
    // Leave vault,
    Leave {},
}

#[cw_ownable_query]
#[cw_serde]
#[derive(QueryResponses)]
pub enum QueryMsg {
    #[returns(Config)]
    Config {},
    // Tells you how much of each vault asset is currently being used (not pending join)
    #[returns(ActiveVaultAssetsResponse)]
    ActiveVaultAssets {},
    // Tells you how much of each vault asset is pending to join for an address
    #[returns(Vec<Coin>)]
    PendingJoin { address: Addr },
    // How much of the vault this address owns
    #[returns(Decimal)]
    VaultRatio { address: Addr },
}

#[cw_serde]
pub struct ActiveVaultAssetsResponse {
    pub asset1: Coin,
    pub asset2: Coin,
}

#[cw_serde]
pub struct MigrateMsg {}
