use std::num::ParseIntError;

use cosmwasm_std::{CheckedFromRatioError, OverflowError, StdError};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ContractError {
    #[error("{0}")]
    Std(#[from] StdError),

    #[error(transparent)]
    OverflowError(#[from] OverflowError),

    #[error(transparent)]
    ParseIntError(#[from] ParseIntError),

    #[error(transparent)]
    CheckedFromRatioError(#[from] CheckedFromRatioError),

    #[error("Pool {} not found", pool_id)]
    PoolNotFound { pool_id: u64 },

    #[error("Pool is not a Concentrated Liquidity Pool")]
    PoolIsNotCL,

    #[error("Commission rate can't be set to 100% or more")]
    CommissionTooHigh,

    #[error("Funds must be sent to participate in the vault")]
    NoFunds,

    #[error("Mint tokens must be asset0 or asset1")]
    InvalidMintAssets,

    #[error("Burn token must be {}", denom)]
    InvalidToken { denom: String },

    #[error("Must redeem at least {} tokens", min)]
    RedemptionBelowMinimum { min: String },

    #[error("Config asset{} is invalid", asset)]
    InvalidConfigAsset { asset: u32 },

    #[error("The assets of the config cannot change")]
    CannotChangeAssets,

    #[error("Trying to add more than available {}{} to position.", amount, asset)]
    CannotAddMoreThanAvailableForAsset { asset: String, amount: String },

    #[error("Operation unauthorized")]
    Unauthorized,

    #[error("Cannot swap more than available of {}", denom)]
    CannotSwapMoreThanAvailable { denom: String },

    #[error("Cannot swap into non vault assets")]
    CannotSwapIntoAsset,

    #[error("Vault cap reached, join not allowed until vault is under cap again")]
    CapReached,

    #[error("Vault halted, nobody can join or leave until unhalted")]
    VaultHalted,

    #[error("Vault closed, nobody can join and funds returned to users")]
    VaultClosed,

    #[error("Can't unlock vault yet. Still {} seconds remaining", seconds)]
    CantUnlockYet { seconds: u64 },

    #[error("No position found")]
    NoPositionsOpen,

    #[error("Amount of {} provided is below minimum", denom)]
    DepositBelowMinimum { denom: String },

    #[error("Deposits for {} are not allowed", denom)]
    DepositNotAllowed { denom: String },

    #[error("Can't remove position, age is less than min uptime")]
    MinUptime,

    #[error("Open CL position found")]
    PositionOpen,

    #[error("Address {} already whitelisted", address)]
    AddressInWhitelist { address: String },

    #[error("Address {} not whitelisted, cannot remove", address)]
    AddressNotInWhitelist { address: String },

    #[error("You can't make someone else exit")]
    CannotForceExit,

    #[error("Account {} is already pending exit", address)]
    AccountPendingBurn { address: String },

    #[error("Insufficient funds to burn")]
    InsufficientFundsToBurn,

    #[error("Insufficient available funds to process burn")]
    CantProcessBurn,

    #[error("Nothing to claim")]
    CannotClaim,
}
