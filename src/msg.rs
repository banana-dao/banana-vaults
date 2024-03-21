use cosmwasm_schema::{cw_serde, QueryResponses};
use cosmwasm_std::{Addr, Coin, Decimal, Uint128};
use osmosis_std_modified::types::osmosis::poolmanager::v1beta1::SwapAmountInSplitRoute;
use pyth_sdk_cw::PriceIdentifier;

use crate::state::{Config, Metadata};

#[cw_serde]
pub struct InstantiateMsg {
    pub metadata: Option<Metadata>,
    // CL Assets with their corresponding pyth price feed
    pub asset0: VaultAsset,
    pub asset1: VaultAsset,
    // Seconds after which a price quote is rejected and entries can't be processed
    pub price_expiry: u64,
    // Must be a CL pool
    pub pool_id: u64,
    // Minimum amount of tokens that can be redeemed in a single tx
    pub min_redemption: Option<Uint128>,
    // USD cap: 1 * 10^(18+8) = 1 USD
    pub dollar_cap: Option<Uint128>,
    // Vault commission, as a percentage
    pub commission: Option<Decimal>,
    // If not specified, receiver will be set to the owner
    pub commission_receiver: Option<Addr>,
    // Used to get the desired pyth contract address - defaults to mainnet
    pub env: Option<Environment>,
    // Vault operator address
    pub operator: Addr,
}

#[cw_serde]
pub enum Environment {
    Mainnet,
    Testnet,
    Testtube,
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

#[cw_serde]
pub enum ExecuteMsg {
    // admin functions
    ManageVault(VaultMsg),
    // main liquidity functions
    ManagePosition(PositionMsg),
    // Join/leave vault
    Deposit(DepositMsg),
    // Dead man switch. Can be called to unlock the vault and allow manual redemptions after 14 days of operator inactivity
    Unlock,
}

#[cw_serde]
pub enum VaultMsg {
    ModifyConfig(Box<Config>),
    ModifyOperator(Addr),
    CollectCommission,
    // Process entries and exits
    ProcessMints,
    ProcessBurns,
    // Manage addresses whitelisted to exceed deposit limits
    Whitelist {
        add: Option<Vec<Addr>>,
        remove: Option<Vec<Addr>>,
    },
    // Halt and Resume deposits and exits
    Halt,
    Resume,
}

#[cw_serde]
pub enum PositionMsg {
    CreatePosition {
        lower_tick: i64,
        upper_tick: i64,
        tokens_provided: Vec<Coin>,
        token_min_amount0: String,
        token_min_amount1: String,
        swap: Option<Swap>,
    },
    AddToPosition {
        position_id: u64,
        amount0: String,
        amount1: String,
        token_min_amount0: String,
        token_min_amount1: String,
        swap: Option<Swap>,
        override_uptime: Option<bool>,
    },
    WithdrawPosition {
        position_id: u64,
        liquidity_amount: String,
        override_uptime: Option<bool>,
    },
}
#[cw_serde]
pub struct Swap {
    pub routes: Vec<SwapAmountInSplitRoute>,
    pub token_in_denom: String,
    pub token_out_min_amount: String,
}

#[cw_serde]
pub enum DepositMsg {
    Mint(Option<Uint128>),
    Burn {
        address: Option<Addr>,
        amount: Option<Uint128>,
    },
}

#[cw_serde]
#[derive(QueryResponses)]
pub enum QueryMsg {
    #[returns(Vec<Coin>)]
    EstimateDeposit(DepositQuery),
    #[returns(Vec<Coin>)]
    LockedAssets,
    #[returns(Vec<AccountResponse>)]
    AccountStatus(AccountQuery),
    #[returns(WhitelistResponse)]
    Whitelist {
        start_after: Option<Addr>,
        limit: Option<u32>,
    },
    #[returns(Status)]
    VaultStatus,
}

#[cw_serde]
pub enum DepositQuery {
    Mint(Vec<Coin>),
    Burn(Uint128),
}

#[cw_serde]
pub enum AccountQuery {
    Mint {
        address: Option<Addr>,
        start_after: Option<Addr>,
        limit: Option<u32>,
    },
    Burn {
        start_after: Option<Addr>,
        limit: Option<u32>,
    },
}

#[cw_serde]
pub struct AccountResponse {
    pub address: Addr,
    pub amount: Vec<Coin>,
    pub min_out: Option<Uint128>,
}

#[cw_serde]
pub struct WhitelistResponse {
    pub whitelisted_depositors: Vec<Addr>,
}

#[cw_serde]
pub struct Status {
    pub join_time: u64,
    pub last_update: u64,
    pub uptime_locked: bool,
    pub cap_reached: bool,
    pub halted: bool,
    pub closed: bool,
    pub owner: Addr,
    pub operator: Addr,
    pub denom: String,
    pub supply: Uint128,
    pub uncompounded_rewards: Vec<Coin>,
    pub uncollected_commission: Vec<Coin>,
    pub config: Config,
}

#[cw_serde]
pub struct MigrateMsg {}
