use std::num::ParseIntError;

use cosmwasm_std::{CheckedFromRatioError, OverflowError, StdError};
use cw_ownable::OwnershipError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ContractError {
    #[error("{0}")]
    Std(#[from] StdError),

    #[error(transparent)]
    Ownership(#[from] OwnershipError),

    #[error(transparent)]
    OverflowError(#[from] OverflowError),

    #[error(transparent)]
    ParseIntError(#[from] ParseIntError),

    #[error(transparent)]
    CheckedFromRatioError(#[from] CheckedFromRatioError),

    #[error("Pool {} not found", pool_id)]
    PoolNotFound { pool_id: u64 },

    #[error("Pool is not a Concentrated Liquidity Pool")]
    PoolIsNotCL {},

    #[error("Funds must be sent to participate in the vault")]
    NoFunds {},

    #[error("You need to send funds that belong to this pool, and not repeat assets")]
    InvalidFunds {},

    #[error("Config asset{} is invalid", asset)]
    InvalidConfigAsset { asset: u32 },

    #[error("The assets of the config cannot change")]
    CannotChangeAssets {},

    #[error("The pool id cannot change")]
    CannotChangePoolId {},

    #[error("Trying to add more than available {}{} to position.", amount, asset)]
    CannotAddMoreThanAvailableForAsset { asset: String, amount: String },

    #[error("Operation unauthorized - only contract can call this function")]
    Unauthorized {},

    #[error("Cannot swap more than available")]
    CannotSwapMoreThanAvailable {},

    #[error("Vault cap reached, join not allowed until vault is under cap again")]
    CapReached {},

    #[error("Vault halted, nobody can join or leave until unhalted")]
    VaultHalted {},

    #[error("Vault closed, nobody can join and funds returned to users")]
    VaultClosed {},

    #[error("Cant force exits yet. Still {} seconds remaining", seconds)]
    CantForceExitsYet { seconds: u64 },

    #[error("No position found")]
    NoPositionsOpen {},

    #[error("Amount of {} provided is below minimum", denom)]
    DepositBelowMinimum { denom: String },

    #[error("Pyth price quote is older than {} seconds, please update", seconds)]
    StalePrice { seconds: u64 },

    #[error("Can't remove position, age is less than min uptime")]
    MinUptime(),

    #[error("Open CL position found")]
    PositionOpen(),

    #[error("Address {} already whitelisted", address)]
    AddressInWhitelist { address: String },

    #[error("Address {} not whitelisted, cannot remove", address)]
    AddressNotInWhitelist { address: String },

    #[error("You can't make someone else exit")]
    CannotForceExit {},

    #[error("Address {} is already pending exit", address)]
    AddressPendingExit { address: String },
}
