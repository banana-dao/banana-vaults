use std::num::ParseIntError;

use cosmwasm_std::{OverflowError, StdError};
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

    #[error("Pool {} not found", pool_id)]
    PoolNotFound { pool_id: u64 },

    #[error("Pool is not a Concentrated Liquidity Pool")]
    PoolIsNotCL {},

    #[error("Two denoms must be sent to participate in the vault")]
    NeedTwoDenoms {},

    #[error("You need to send funds that belong to this pool")]
    InvalidFunds {},

    #[error("The assets you sent in the message are not in this CL pool")]
    InvalidConfigAsset {},

    #[error("The assets of the config cannot change")]
    CannotChangeAssets {},

    #[error("The pool id cannot change")]
    CannotChangePoolId {},

    #[error("Trying to add more than available of {} to position", asset)]
    CannotAddMoreThenAvailableForAsset { asset: String },

    #[error("Operation unauthorized - only contract can call this function")]
    Unauthorized {},

    #[error("User already in exit list - wait for exit")]
    UserAlreadyInExitList {},

    #[error("Cannot swap more than available")]
    CannotSwapMoreThanAvailable {},
}
