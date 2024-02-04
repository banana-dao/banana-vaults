use cosmwasm_schema::{cw_serde, QueryResponses};
use cosmwasm_std::{Addr, Coin, Decimal, Uint128};
use cw_ownable::{cw_ownable_execute, cw_ownable_query};
use osmosis_std_modified::types::osmosis::poolmanager::v1beta1::SwapAmountInSplitRoute;
use pyth_sdk_cw::PriceIdentifier;

use crate::state::Config;

#[cw_serde]
pub struct InstantiateMsg {
    // Some metadata about the vault
    pub name: String,
    pub description: Option<String>,
    pub image: Option<String>,
    // Must be a CL pool
    pub pool_id: u64,
    // Update users frequency (adding users that want to join and removing users that want to leave)
    pub update_frequency: Frequency,
    // Seconds after which a price quote is rejected and joins/leaves can't be processed
    pub price_expiry: u64,
    //  Uptime must be enforced to accurately calculate incentives
    pub min_uptime: Option<u64>,
    // CL Assets with their corresponding pyth price feed
    pub asset0: VaultAsset,
    pub asset1: VaultAsset,
    pub dollar_cap: Option<Uint128>, // with 8 decimals. Example: If vault cap is 50k USD we pass here 50000 * 10^8 = "5000000000000"
    // Vault commission (in %)
    pub commission: Option<Decimal>,
    // If no address specified, contract admin will be receiver of commissions
    pub commission_receiver: Option<Addr>,
    // Addresses allowed to exceed the deposit cap
    pub whitelisted_depositors: Option<Vec<Addr>>,
    // Flag to take the right pyth contract address - true for mainnet, false for testnet
    pub mainnet: bool,
    // Vault operator address
    pub operator: Addr,
}

#[cw_serde]
pub enum Frequency {
    Blocks(u64),
    Seconds(u64),
}

#[cw_serde]
pub struct VaultAsset {
    pub denom: String,
    // Pyth asset id
    pub price_identifier: PriceIdentifier,
    // Need to know decimals to convert from pyth price to asset price
    pub decimals: u32,
    // The minimum amount of tokens that can be deposited in a single tx
    pub min_deposit: Uint128,
}

#[cw_ownable_execute]
#[cw_serde]
pub enum ExecuteMsg {
    // If for some reason the pyth oracle contract address or the price identifiers change, we can update it (also for testing)
    ModifyConfig {
        config: Box<Config>,
    },
    // Create position
    CreatePosition {
        lower_tick: i64,
        upper_tick: i64,
        tokens_provided: Vec<Coin>,
        token_min_amount0: String,
        token_min_amount1: String,
        swap: Option<Swap>,
    },
    // Add to position
    AddToPosition {
        position_id: u64,
        amount0: String,
        amount1: String,
        token_min_amount0: String,
        token_min_amount1: String,
        swap: Option<Swap>,
    },
    // Withdraw position
    WithdrawPosition {
        position_id: u64,
        liquidity_amount: String,
    },
    // Process entries and exits (done internally by the contract every update frequency)
    ProcessNewEntriesAndExits {},
    // Join vault
    Join {},
    // Leave vault,
    Leave {},
    // Halt and Resume for Admin
    Halt {},
    Resume {},
    // Close vault. If this is triggered the vault will be closed, nobody else can join and all funds will be withdrawn and sent to the users during next update
    CloseVault {},
    // Dead man switch
    ForceExits {},
}

#[cw_serde]
pub struct Swap {
    pub routes: Vec<SwapAmountInSplitRoute>,
    pub token_in_denom: String,
    pub token_out_min_amount: String,
}

#[cw_ownable_query]
#[cw_serde]
#[derive(QueryResponses)]
pub enum QueryMsg {
    #[returns(Config)]
    Config {},
    // Tells you how much of each vault asset is currently being used (not pending join)
    #[returns(TotalAssetsResponse)]
    TotalActiveAssets {},
    // Tells you how much is pending join in total
    #[returns(TotalAssetsResponse)]
    TotalPendingAssets {},
    // Tells you how much of each vault asset is pending to join for an address
    #[returns(Vec<Coin>)]
    PendingJoin { address: Addr },
    // How much of the vault this address owns
    #[returns(Decimal)]
    VaultRatio { address: Addr },
}

#[cw_serde]
pub struct TotalAssetsResponse {
    pub asset0: Coin,
    pub asset1: Coin,
}

#[cw_serde]
pub struct MigrateMsg {}
