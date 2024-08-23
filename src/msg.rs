use cosmwasm_schema::{cw_serde, QueryResponses};
use cosmwasm_std::{Addr, Coin, Decimal, Uint128};
use osmosis_std::types::osmosis::poolmanager::v1beta1::SwapAmountInSplitRoute;
use pyth_sdk_cw::PriceIdentifier;

use crate::state::{Config, Metadata};

#[cw_serde]
pub struct InstantiateMsg {
    pub metadata: Option<Metadata>,
    // CL Assets with their corresponding pyth price feed
    pub asset0: VaultAsset,
    pub asset1: VaultAsset,
    // Minimum amount of tokens that can be deposited in a single tx
    pub min_asset0: Uint128,
    pub min_asset1: Uint128,
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
    // Modify the vault config
    Modify(ModifyMsg),
    CompoundRewards(Vec<Swap>),
    CollectCommission,
    // Process entries and exits
    ProcessMints,
    ProcessBurns,
    // Halt and Resume deposits and exits
    Halt,
    Resume,
}

#[cw_serde]
pub enum ModifyMsg {
    Operator(Addr),
    Config(Box<Config>),
    PoolId(u64),
    Commission(Decimal),
    Whitelist {
        add: Option<Vec<Addr>>,
        remove: Option<Vec<Addr>>,
    },
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
pub enum DepositMsg {
    Mint {
        min_out: Option<Uint128>,
    },
    Burn {
        address: Option<Addr>,
        amount: Option<Uint128>,
    },
}

#[cw_serde]
pub struct Swap {
    pub routes: Vec<SwapAmountInSplitRoute>,
    pub token_in_denom: String,
    pub token_out_min_amount: String,
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
    #[returns(Vec<Coin>)]
    Rewards(RewardQuery),
    #[returns(WhitelistResponse)]
    Whitelist {
        start_after: Option<Addr>,
        limit: Option<u32>,
    },
    #[returns(State)]
    VaultState(StateQuery),
}

#[cw_serde]
pub enum DepositQuery {
    Mint(Vec<Coin>),
    Burn(Uint128),
}

#[cw_serde]
pub enum AccountQuery {
    Mint(AccountQueryParams),
    Burn(AccountQueryParams),
}

#[cw_serde]
pub struct AccountQueryParams {
    pub address: Option<Addr>,
    pub start_after: Option<Addr>,
    pub limit: Option<u32>,
}

#[cw_serde]
pub struct AccountResponse {
    pub address: Addr,
    pub amount: Vec<Coin>,
    pub min_out: Option<Uint128>,
}

#[cw_serde]
pub enum RewardQuery {
    Commission,
    Uncompounded,
}

#[cw_serde]
pub struct WhitelistResponse {
    pub whitelisted_depositors: Vec<Addr>,
}

#[cw_serde]
pub enum State {
    Info {
        asset0: VaultAsset,
        asset1: VaultAsset,
        pool_id: u64,
        owner: Addr,
        operator: Addr,
        commission_rate: Decimal,
        config: Box<Config>,
    },
    Status {
        join_time: u64,
        last_update: u64,
        uptime_locked: bool,
        cap_reached: bool,
        halted: bool,
        terminated: bool,
        supply: Uint128,
        denom: String,
    },
}

#[cw_serde]
pub enum StateQuery {
    Info,
    Status,
}

#[cw_serde]
pub struct MigrateMsg {}
